//! Time stretch by WSOLA (TDStretch, float build, plain non-SSE kernels).
//!
//! The input is cut into sequences; each new one is moved within a seek
//! window to where it best matches (highest normalised cross-correlation)
//! the tail of the previous one, then cross-faded over the overlap. The
//! sequence advance is `tempo` times the output advance.

use crate::fifo::Fifo;

/// Candidate positions scored side by side. Each lane is one position,
/// still summed tap by tap in SoundTouch's order, so the scores are
/// bit-identical to its scalar loops; the lanes only let the compiler
/// vectorise across positions. The full search (correlation only) uses
/// more lanes than the quick one (correlation and norm).
const FULL_LANES: usize = 32;
const QUICK_LANES: usize = 16;
/// The quick search's coarse step.
const SCANSTEP: usize = 16;
const SCANWIND: usize = 8;
/// Slack after each per-channel row, so a full-search batch may run past
/// the window (the extra lanes are ignored).
const ROW_PAD: usize = FULL_LANES;

// Automatic sequence / seek-window lengths, linear in the tempo between
// these points and clamped outside them.
const AUTOSEQ_TEMPO_LOW: f64 = 0.5;
const AUTOSEQ_TEMPO_TOP: f64 = 2.0;
const AUTOSEQ_AT_MIN: f64 = 90.0;
const AUTOSEQ_AT_MAX: f64 = 40.0;
const AUTOSEQ_K: f64 = (AUTOSEQ_AT_MAX - AUTOSEQ_AT_MIN) / (AUTOSEQ_TEMPO_TOP - AUTOSEQ_TEMPO_LOW);
const AUTOSEQ_C: f64 = AUTOSEQ_AT_MIN - AUTOSEQ_K * AUTOSEQ_TEMPO_LOW;
const AUTOSEEK_AT_MIN: f64 = 20.0;
const AUTOSEEK_AT_MAX: f64 = 15.0;
const AUTOSEEK_K: f64 = (AUTOSEEK_AT_MAX - AUTOSEEK_AT_MIN) / (AUTOSEQ_TEMPO_TOP - AUTOSEQ_TEMPO_LOW);
const AUTOSEEK_C: f64 = AUTOSEEK_AT_MIN - AUTOSEEK_K * AUTOSEQ_TEMPO_LOW;

const DEFAULT_OVERLAP_MS: i32 = 8;

/// `sample_rate * ms / 1000` in integers, as SoundTouch computes it (but
/// without its overflow on absurd values).
fn ms_to_frames(sample_rate: i32, ms: i32) -> i32 {
    (sample_rate as i64 * ms as i64 / 1000).clamp(0, i32::MAX as i64 / 4) as i32
}

fn check_limits(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

pub(crate) struct TdStretch {
    channels: usize,
    sample_req: i32,
    overlap_length: i32,
    seek_length: i32,
    seek_window_length: i32,
    sample_rate: i32,
    sequence_ms: i32,
    seek_window_ms: i32,
    overlap_ms: i32,
    tempo: f64,
    nominal_skip: f64,
    skip_fract: f64,
    quick_seek: bool,
    auto_seq: bool,
    auto_seek: bool,
    is_beginning: bool,
    mid: Vec<f32>,       // overlap_length frames: the tail of the last sequence
    planar: Vec<f32>,    // the search region split per channel, padded rows
    decimated: Vec<f32>, // the same split by position mod 16 (coarse pass)
    pub input: Fifo,
    pub output: Fifo,
}

impl TdStretch {
    /// The state SoundTouch reaches after its constructor and
    /// `setSampleRate(sample_rate)`. `sample_rate` must be 1..=192000.
    pub fn new(sample_rate: i32, channels: usize) -> Self {
        let mut td = TdStretch {
            channels,
            sample_req: 0,
            overlap_length: 0,
            seek_length: 0,
            seek_window_length: 0,
            sample_rate: 44100,
            sequence_ms: 0,
            seek_window_ms: 0,
            overlap_ms: DEFAULT_OVERLAP_MS,
            tempo: 1.0,
            nominal_skip: 0.0,
            skip_fract: 0.0,
            quick_seek: false,
            auto_seq: true,
            auto_seek: true,
            is_beginning: true,
            mid: Vec::new(),
            planar: Vec::new(),
            decimated: Vec::new(),
            input: Fifo::new(channels),
            output: Fifo::new(channels),
        };
        td.set_parameters(44100, 0, 0, DEFAULT_OVERLAP_MS);
        td.set_tempo(1.0);
        td.clear();
        td.set_parameters(sample_rate, -1, -1, -1);
        td
    }

    pub fn reserve(&mut self, frames: usize) {
        self.input.reserve(frames + self.sample_req.max(0) as usize);
        self.output.reserve(frames);
    }

    /// Positive values replace the setting; 0 makes the sequence / seek
    /// window automatic again; negative values keep the current one.
    pub fn set_parameters(&mut self, sample_rate: i32, sequence_ms: i32, seek_window_ms: i32, overlap_ms: i32) {
        if sample_rate > 0 {
            self.sample_rate = sample_rate.min(192_000);
        }
        if overlap_ms > 0 {
            self.overlap_ms = overlap_ms;
        }
        if sequence_ms > 0 {
            self.sequence_ms = sequence_ms;
            self.auto_seq = false;
        } else if sequence_ms == 0 {
            self.auto_seq = true;
        }
        if seek_window_ms > 0 {
            self.seek_window_ms = seek_window_ms;
            self.auto_seek = false;
        } else if seek_window_ms == 0 {
            self.auto_seek = true;
        }
        self.calc_seq_parameters();
        self.calculate_overlap_length(self.overlap_ms);
        self.set_tempo(self.tempo);
    }

    /// (sample rate, sequence ms, seek window ms, overlap ms), 0 for the
    /// automatic lengths.
    pub fn parameters(&self) -> (i32, i32, i32, i32) {
        (
            self.sample_rate,
            if self.auto_seq { 0 } else { self.sequence_ms },
            if self.auto_seek { 0 } else { self.seek_window_ms },
            self.overlap_ms,
        )
    }

    pub fn set_quick_seek(&mut self, on: bool) {
        self.quick_seek = on;
    }

    pub fn quick_seek(&self) -> bool {
        self.quick_seek
    }

    pub fn set_tempo(&mut self, tempo: f64) {
        self.tempo = tempo;
        self.calc_seq_parameters();
        self.nominal_skip = tempo * (self.seek_window_length - self.overlap_length) as f64;
        let int_skip = (self.nominal_skip + 0.5) as i32;
        self.sample_req = (int_skip + self.overlap_length).max(self.seek_window_length) + self.seek_length;
    }

    /// Input frames needed before the next sequence comes out.
    pub fn latency(&self) -> i32 {
        self.sample_req
    }

    pub fn input_sample_req(&self) -> i32 {
        (self.nominal_skip + 0.5) as i32
    }

    pub fn output_batch_size(&self) -> i32 {
        self.seek_window_length - self.overlap_length
    }

    pub fn clear(&mut self) {
        self.output.clear();
        self.clear_input();
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.mid.fill(0.0);
        self.is_beginning = true;
        self.skip_fract = 0.0;
    }

    pub fn put_samples(&mut self, samples: &[f32]) {
        self.input.put(samples);
        self.process();
    }

    fn calc_seq_parameters(&mut self) {
        if self.auto_seq {
            let seq = check_limits(AUTOSEQ_C + AUTOSEQ_K * self.tempo, AUTOSEQ_AT_MAX, AUTOSEQ_AT_MIN);
            self.sequence_ms = (seq + 0.5) as i32;
        }
        if self.auto_seek {
            let seek = check_limits(AUTOSEEK_C + AUTOSEEK_K * self.tempo, AUTOSEEK_AT_MAX, AUTOSEEK_AT_MIN);
            self.seek_window_ms = (seek + 0.5) as i32;
        }
        self.seek_window_length = ms_to_frames(self.sample_rate, self.sequence_ms);
        if self.seek_window_length < 2 * self.overlap_length {
            self.seek_window_length = 2 * self.overlap_length;
        }
        self.seek_length = ms_to_frames(self.sample_rate, self.seek_window_ms);
    }

    fn calculate_overlap_length(&mut self, overlap_ms: i32) {
        let mut ovl = ms_to_frames(self.sample_rate, overlap_ms);
        if ovl < 16 {
            ovl = 16;
        }
        ovl -= ovl % 8;
        // A longer overlap gets a fresh, silent buffer; a shorter one keeps
        // the start of the old one, as SoundTouch does.
        let prev = self.overlap_length;
        self.overlap_length = ovl;
        let len = ovl as usize * self.channels;
        if ovl > prev {
            self.mid.clear();
            self.mid.resize(len, 0.0);
        } else {
            self.mid.truncate(len);
        }
    }

    fn process(&mut self) {
        let ch = self.channels;
        let ovl = self.overlap_length as usize;
        while self.input.len() as i64 >= self.sample_req as i64 {
            let mut offset = 0usize;
            if !self.is_beginning {
                offset = self.seek_best_overlap_position();
                let input = &self.input.data()[ch * offset..];
                let out = self.output.tail(ovl);
                if ch == 1 {
                    overlap_mono(out, input, &self.mid, ovl);
                } else {
                    overlap_multi(out, input, &self.mid, ovl, ch);
                }
                self.output.commit(ovl);
                offset += ovl;
            } else {
                // No overlap at the very start; the skip makes up for it.
                self.is_beginning = false;
                let skip = (self.tempo * self.overlap_length as f64 + 0.5 * self.seek_length as f64 + 0.5) as i32;
                self.skip_fract -= skip as f64;
                if self.skip_fract <= -self.nominal_skip {
                    self.skip_fract = -self.nominal_skip;
                }
            }

            // Cannot happen (sample_req covers it); SoundTouch would retry
            // forever here, this stops instead.
            if (self.input.len() as i64) < (offset as i64 + self.seek_window_length as i64 - ovl as i64) {
                break;
            }

            let seq = (self.seek_window_length - 2 * self.overlap_length).max(0) as usize;
            let data = self.input.data();
            self.output.put(&data[ch * offset..ch * (offset + seq)]);
            self.mid.copy_from_slice(&data[ch * (offset + seq)..ch * (offset + seq + ovl)]);

            self.skip_fract += self.nominal_skip;
            let ovl_skip = self.skip_fract as i32;
            self.skip_fract -= ovl_skip as f64;
            // A negative skip would wrap to "everything" in SoundTouch.
            self.input.skip(ovl_skip as u32 as usize);
        }
    }

    fn seek_best_overlap_position(&mut self) -> usize {
        if self.quick_seek { self.seek_quick() } else { self.seek_full() }
    }

    /// Every position of the seek window. The norm of the candidate is
    /// rolled along in double (calcCrossCorrAccumulate), exactly as the
    /// plain (non-SSE) upstream build does it.
    fn seek_full(&mut self) -> usize {
        let ch = self.channels;
        let ovl = self.overlap_length as usize;
        let ilen = ch * ovl;
        let seek_len = self.seek_length.max(0) as usize;
        if seek_len <= 1 {
            return 0; // only position 0
        }
        let span = self.fill_planar(seek_len + ovl);
        let refp = self.input.data();
        let mid = &self.mid[..ilen];

        let mut norm = 0.0f64;
        let mut best_corr = 0.0f64;
        let mut best_offs = 0usize;
        let mut i = 0usize;
        while i < seek_len {
            let n = FULL_LANES.min(seek_len - i);
            let corrs = corr_lanes(&self.planar, span, mid, ch, ovl, i);
            for (l, &corr_raw) in corrs[..n].iter().enumerate() {
                let pos = i + l;
                if pos == 0 {
                    // calcCrossCorr, with the norm summed in float.
                    let (_, norms) = corr_norm_lanes::<1>(&self.planar, span, mid, ch, ovl, 0);
                    norm = norms[0] as f64;
                    best_corr = (normalise(corr_raw, norms[0]) + 0.1) * 0.75;
                    continue;
                }
                let p = ch * pos;
                for k in 1..=ch {
                    let v = refp[p - k];
                    norm -= (v * v) as f64;
                }
                for k in 1..=ch {
                    let v = refp[p + ilen - k];
                    norm += (v * v) as f64;
                }
                let corr = corr_raw as f64 / (if norm < 1e-9 { 1.0 } else { norm }).sqrt();
                let tmp = (2 * pos as i32 - self.seek_length) as f64 / self.seek_length as f64;
                let corr = (corr + 0.1) * (1.0 - 0.25 * tmp * tmp);
                if corr > best_corr {
                    best_corr = corr;
                    best_offs = pos;
                }
            }
            i += n;
        }
        best_offs
    }

    /// A coarse pass every 16 positions, then a fine pass around the two
    /// best (TDStretch::seekBestOverlapPositionQuick), in float.
    fn seek_quick(&mut self) -> usize {
        let seek_len = self.seek_length.max(0) as usize;
        let frames = seek_len + self.overlap_length as usize;
        let span = self.fill_planar(frames);
        let dspan = self.fill_decimated(frames);
        let score = |corr: f64, i: usize| -> f32 {
            let tmp = (2 * i as i32 - seek_len as i32 - 1) as f32 / seek_len as f32;
            (corr as f32 + 0.1) * (1.0 - 0.25 * tmp * tmp)
        };

        let mut best_corr = -f32::MAX;
        let mut best_corr2 = -f32::MAX;
        let mut best_offs = SCANWIND;
        let mut best_offs2 = SCANWIND;
        let limit = seek_len as isize - SCANWIND as isize - 1;
        let mut i = SCANSTEP;
        while (i as isize) < limit {
            let n = ((limit as usize - i).div_ceil(SCANSTEP)).min(QUICK_LANES);
            let corrs = self.corr_set_coarse(dspan, i);
            for (l, &c) in corrs[..n].iter().enumerate() {
                let pos = i + SCANSTEP * l;
                let corr = score(c, pos);
                if corr > best_corr {
                    best_corr2 = best_corr;
                    best_offs2 = best_offs;
                    best_corr = corr;
                    best_offs = pos;
                } else if corr > best_corr2 {
                    best_corr2 = corr;
                    best_offs2 = pos;
                }
            }
            i += SCANSTEP * n;
        }

        // The surroundings of both, the already scored centre skipped: two
        // runs of at most 8 positions each, in increasing order.
        for centre in [best_offs, best_offs2] {
            let end = (centre + SCANWIND + 1).min(seek_len);
            for (first, last) in [(centre - SCANWIND, centre.min(end)), (centre + 1, end)] {
                if first >= last {
                    continue;
                }
                let corrs = self.corr_run(span, first);
                for (l, &c) in corrs[..last - first].iter().enumerate() {
                    let corr = score(c, first + l);
                    if corr > best_corr {
                        best_corr = corr;
                        best_offs = first + l;
                    }
                }
            }
        }
        best_offs
    }

    /// calcCrossCorr of the SCANWIND positions from `first`.
    fn corr_run(&self, span: usize, first: usize) -> [f64; SCANWIND] {
        let ch = self.channels;
        let ovl = self.overlap_length as usize;
        let (corr, norm) = corr_norm_lanes::<SCANWIND>(&self.planar, span, &self.mid[..ch * ovl], ch, ovl, first);
        std::array::from_fn(|l| normalise(corr[l], norm[l]))
    }

    /// calcCrossCorr of positions `first + SCANSTEP * l` (`first` a
    /// multiple of SCANSTEP), from the decimated rows.
    fn corr_set_coarse(&self, dspan: usize, first: usize) -> [f64; QUICK_LANES] {
        let ch = self.channels;
        let ovl = self.overlap_length as usize;
        let mut corr = [0.0f32; QUICK_LANES];
        let mut norm = [0.0f32; QUICK_LANES];
        for (j, m) in self.mid[..ch * ovl].chunks_exact(ch).enumerate() {
            for (c, &m) in m.iter().enumerate() {
                // Position first + 16 l + j sits in row (c, j % 16) at
                // first / 16 + l + j / 16.
                let at = (c * SCANSTEP + j % SCANSTEP) * dspan + first / SCANSTEP + j / SCANSTEP;
                let row = &self.decimated[at..at + QUICK_LANES];
                for l in 0..QUICK_LANES {
                    let v = row[l];
                    corr[l] += v * m;
                    norm[l] += v * v;
                }
            }
        }
        std::array::from_fn(|l| normalise(corr[l], norm[l]))
    }

    /// The first `frames` input frames by channel and position mod 16, for
    /// the coarse pass; returns the row stride.
    fn fill_decimated(&mut self, frames: usize) -> usize {
        let ch = self.channels;
        let dspan = frames / SCANSTEP + QUICK_LANES + 2;
        if self.decimated.len() < dspan * SCANSTEP * ch {
            self.decimated.resize(dspan * SCANSTEP * ch, 0.0);
        }
        let src = &self.input.data()[..frames * ch];
        for (f, frame) in src.chunks_exact(ch).enumerate() {
            let (q, r) = (f / SCANSTEP, f % SCANSTEP);
            for (c, &x) in frame.iter().enumerate() {
                self.decimated[(c * SCANSTEP + r) * dspan + q] = x;
            }
        }
        dspan
    }

    /// Copies the first `frames` input frames into one row per channel, so
    /// the lanes read contiguous memory; returns the row stride. The rows
    /// are padded so any batch starting inside the window stays in bounds.
    fn fill_planar(&mut self, frames: usize) -> usize {
        let ch = self.channels;
        let span = frames + ROW_PAD;
        if self.planar.len() < span * ch {
            self.planar.resize(span * ch, 0.0);
        }
        let src = &self.input.data()[..frames * ch];
        for (c, row) in self.planar.chunks_exact_mut(span).take(ch).enumerate() {
            for (d, s) in row[..frames].iter_mut().zip(src[c..].iter().step_by(ch)) {
                *d = *s;
            }
        }
        span
    }
}

/// `corr / sqrt(norm)`, in double, with a silent candidate taken as norm 1.
fn normalise(corr: f32, norm: f32) -> f64 {
    corr as f64 / (if (norm as f64) < 1e-9 { 1.0 } else { norm as f64 }).sqrt()
}

/// Correlations and norms, summed in float, of positions `first..first + N`
/// from the per-channel rows.
fn corr_norm_lanes<const N: usize>(
    planar: &[f32],
    span: usize,
    mid: &[f32],
    ch: usize,
    ovl: usize,
    first: usize,
) -> ([f32; N], [f32; N]) {
    let mut corr = [0.0f32; N];
    let mut norm = [0.0f32; N];
    for (j, m) in mid.chunks_exact(ch).take(ovl).enumerate() {
        for (c, &m) in m.iter().enumerate() {
            let at = c * span + first + j;
            let row = &planar[at..at + N];
            for l in 0..N {
                let v = row[l];
                corr[l] += v * m;
                norm[l] += v * v;
            }
        }
    }
    (corr, norm)
}

/// The un-normalised correlations of positions `first..first + FULL_LANES`.
/// The interleaved sum over k = j*ch + c becomes, per lane, the same
/// sequence of float additions read from the per-channel rows.
fn corr_lanes(planar: &[f32], span: usize, mid: &[f32], ch: usize, ovl: usize, first: usize) -> [f32; FULL_LANES] {
    let mut acc = [0.0f32; FULL_LANES];
    match ch {
        1 => {
            let row = &planar[first..first + ovl + FULL_LANES - 1];
            for (w, &m) in row.windows(FULL_LANES).zip(&mid[..ovl]) {
                for l in 0..FULL_LANES {
                    acc[l] += w[l] * m;
                }
            }
        }
        2 => {
            let left = &planar[first..first + ovl + FULL_LANES - 1];
            let right = &planar[span + first..span + first + ovl + FULL_LANES - 1];
            for ((wl, wr), m) in left.windows(FULL_LANES).zip(right.windows(FULL_LANES)).zip(mid.as_chunks::<2>().0) {
                for l in 0..FULL_LANES {
                    acc[l] += wl[l] * m[0];
                }
                for l in 0..FULL_LANES {
                    acc[l] += wr[l] * m[1];
                }
            }
        }
        _ => {
            // Two halves, so the accumulators stay in registers.
            const HALF: usize = FULL_LANES / 2;
            for (h, acc) in acc.as_chunks_mut::<HALF>().0.iter_mut().enumerate() {
                let mut a = [0.0f32; HALF];
                for (j, m) in mid.chunks_exact(ch).take(ovl).enumerate() {
                    for (c, &m) in m.iter().enumerate() {
                        let at = c * span + first + h * HALF + j;
                        let w = &planar[at..at + HALF];
                        for l in 0..HALF {
                            a[l] += w[l] * m;
                        }
                    }
                }
                acc.copy_from_slice(&a);
            }
        }
    }
    acc
}

fn overlap_mono(out: &mut [f32], input: &[f32], mid: &[f32], ovl: usize) {
    let mut m1 = 0.0f32;
    let mut m2 = ovl as f32;
    let div = ovl as f32;
    for ((o, &x), &y) in out[..ovl].iter_mut().zip(&input[..ovl]).zip(&mid[..ovl]) {
        *o = (x * m1 + y * m2) / div;
        m1 += 1.0;
        m2 -= 1.0;
    }
}

/// Stereo and multichannel share this arithmetic.
fn overlap_multi(out: &mut [f32], input: &[f32], mid: &[f32], ovl: usize, ch: usize) {
    let scale = 1.0f32 / ovl as f32;
    let mut f1 = 0.0f32;
    let mut f2 = 1.0f32;
    let n = ovl * ch;
    for ((o, x), y) in out[..n].chunks_exact_mut(ch).zip(input[..n].chunks_exact(ch)).zip(mid[..n].chunks_exact(ch)) {
        for c in 0..ch {
            o[c] = x[c] * f1 + y[c] * f2;
        }
        f1 += scale;
        f2 -= scale;
    }
}
