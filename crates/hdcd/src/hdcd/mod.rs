//! HDCD decoding: a bit-exact port of libhdcd 1.4 (hdcd_simple.c,
//! hdcd_decode2.c, hdcd_analyze_tonegen.c, hdcd_strings.c), as Sonkore uses it:
//!
//! - a track is decoded only when it is 16-bit stereo and its first 5 s carry
//!   HDCD codes ([`detect`]);
//! - the decoder is reset per track and per seek with `reset_ext(rate, 16)`;
//! - it takes the decoder's left-justified s32 samples `>> 16` and gives 32-bit
//!   samples at -6 dB, so peak extension has room above the old full scale
//!   ([`Hdcd::process_s32`]).
//!
//! libhdcd's optional logging callback is left out: Sonkore keeps it disabled,
//! and it never changes the output. Its counters are in [`ChannelStats`].

mod tables;

use tables::{GAINTAB, PE_MAX_ASAMPLE, PEAK_EXT_LEVEL, PEAKTAB, READAHEADTAB, TONE_TAB_176400, TONE_TAB_192000};

const FLAG_FORCE_PE: i32 = 128;
const FLAG_TGM_LOG_OFF: i32 = 64;

/// Is HDCD encoding detected? (`hdcd_dv`)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detected {
    /// HDCD packets do not (yet) appear.
    #[default]
    None = 0,
    /// HDCD packets appear, but all control codes are NOP.
    NoEffect = 1,
    /// HDCD packets appear, and change the output in some way.
    Effectual = 2,
}

impl Detected {
    /// `hdcd_str_detect`.
    pub fn as_str(self) -> &'static str {
        match self {
            Detected::None => "not detected",
            Detected::NoEffect => "detected (no effect)",
            Detected::Effectual => "detected",
        }
    }
}

/// How peak extend was used by the valid packets (`hdcd_pe`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PeakExtend {
    /// All valid packets have PE off.
    #[default]
    Never = 0,
    /// Some valid packets have PE on.
    Intermittent = 1,
    /// All valid packets have PE on.
    Permanent = 2,
}

impl PeakExtend {
    /// `hdcd_str_pe`.
    pub fn as_str(self) -> &'static str {
        match self {
            PeakExtend::Never => "never enabled",
            PeakExtend::Intermittent => "enabled intermittently",
            PeakExtend::Permanent => "enabled permanently",
        }
    }
}

/// The packet formats found (`hdcd_pf`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PacketFormat {
    /// No packets discovered.
    #[default]
    None = 0,
    /// Type A: 8-bit control.
    A = 1,
    /// Type B: 8-bit control, 8-bit XOR.
    B = 2,
    /// Both A and B, most likely an error.
    Mix = 3,
}

impl PacketFormat {
    fn from_bits(v: u8) -> Self {
        match v & 3 {
            0 => PacketFormat::None,
            1 => PacketFormat::A,
            2 => PacketFormat::B,
            _ => PacketFormat::Mix,
        }
    }

    /// `hdcd_str_pformat`.
    pub fn as_str(self) -> &'static str {
        match self {
            PacketFormat::None => "?",
            PacketFormat::A => "A",
            PacketFormat::B => "B",
            PacketFormat::Mix => "A+B",
        }
    }
}

/// Analyze modes (`hdcd_ana_mode`): the audio is replaced by a solid tone
/// whose amplitude signals where a feature is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AnalyzeMode {
    #[default]
    Off = 0,
    /// Gain adjustment level at each sample.
    Lle = 1,
    /// Samples where peak extend occurs.
    Pe = 2,
    /// Samples where the code detect timer is active.
    Cdt = 3,
    /// Samples where the target gain does not match between channels.
    Tgm = 4,
    /// Any samples above peak extend level.
    Pel = 5,
    /// Gain adjustment level at each sample, in each channel.
    Ltgm = 6,
}

impl AnalyzeMode {
    /// `hdcd_str_analyze_mode_desc`.
    pub fn description(self) -> &'static str {
        match self {
            AnalyzeMode::Off => "disabled",
            AnalyzeMode::Lle => "gain adjustment level at each sample",
            AnalyzeMode::Pe => "samples where peak extend occurs",
            AnalyzeMode::Cdt => "samples where the code detect timer is active",
            AnalyzeMode::Tgm => "samples where the target gain does not match between channels",
            AnalyzeMode::Pel => "any samples above peak extend level",
            AnalyzeMode::Ltgm => "gain adjustment level at each sample, in each channel",
        }
    }
}

/// What libhdcd reports after processing (`hdcd_detection_data` and the
/// `hdcd_detect_*` getters).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Detection {
    pub detected: Detected,
    pub packet_type: PacketFormat,
    /// Valid packets.
    pub total_packets: i32,
    /// Detectable errors.
    pub errors: i32,
    pub peak_extend: PeakExtend,
    /// The transient filter flag was seen.
    pub uses_transient_filter: bool,
    /// In dB, expected in -7.5..=0.0.
    pub max_gain_adjustment: f32,
    /// -1: the code detect timer was never set; 0: set, never expired.
    pub cdt_expirations: i32,
    /// Samples whose target gain did not match between the channels.
    pub lle_mismatch: i32,
}

impl Detection {
    /// `hdcd_detect_str`: "HDCD detected: yes (B:1234), peak_extend: …".
    pub fn summary(&self) -> String {
        if self.detected != Detected::None {
            format!(
                "HDCD detected: yes ({}:{}), peak_extend: {}, max_gain_adj: {:.1} dB, transient_filter: {}, detectable errors: {}",
                self.packet_type.as_str(),
                self.total_packets,
                self.peak_extend.as_str(),
                self.max_gain_adjustment,
                if self.uses_transient_filter { "detected" } else { "not detected" },
                self.errors
            )
        } else {
            "HDCD detected: no".to_owned()
        }
    }
}

/// One channel's counters (what `hdcd_logger_dump_state` prints).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChannelStats {
    /// 8-bit format packets.
    pub code_counter_a: i32,
    /// Looked like an A code, but a bit expected to be 0 was 1.
    pub code_counter_a_almost: i32,
    /// 16-bit format packets (8-bit code, 8-bit XOR).
    pub code_counter_b: i32,
    /// Looked like a B code, but failed the XOR check.
    pub code_counter_b_checkfails: i32,
    /// Packet prefixes found.
    pub code_counter_c: i32,
    pub code_counter_c_unmatched: i32,
    /// Valid packets with peak extend on.
    pub count_peak_extend: i32,
    /// Valid packets with the transient filter flag.
    pub count_transient_filter: i32,
    /// Valid packets per target gain (0 to -7.5 dB in 0.5 dB steps).
    pub gain_counts: [i32; 16],
    pub max_gain: i32,
    /// Code detect timer expirations; -1 for never set.
    pub count_sustain_expired: i32,
}

// --- decoder state (hdcd_decode2.h) -------------------------------------------------

#[derive(Clone, Debug, Default)]
struct State {
    decoder_options: i32,
    window: u64,
    readahead: u8,
    arg: u8,
    control: u8,
    sustain: u32,
    sustain_reset: u32,
    running_gain: i32,
    bits: i32,
    rate: i32,
    #[allow(dead_code)] // libhdcd keeps it, never reads it
    cdt_period: i32,
    stats: ChannelStats,
    sample_count: i32,
    ana_mode: AnalyzeMode,
    ana_snb: i32,
}

impl State {
    /// `_hdcd_reset`
    fn reset(rate: u32, bits: u32, sustain_period_ms: i32, flags: i32) -> Self {
        let rate = if rate == 0 { 44100 } else { rate };
        let bits = if bits == 0 { 16 } else { bits };
        let sustain_period_ms = if sustain_period_ms == 0 { 2000 } else { sustain_period_ms.clamp(100, 60000) };
        let sustain_reset = (sustain_period_ms as u32).wrapping_mul(rate / 100) / 10;
        State {
            decoder_options: flags,
            cdt_period: sustain_period_ms,
            rate: rate as i32,
            bits: bits as i32,
            readahead: 32,
            sustain_reset,
            stats: ChannelStats { count_sustain_expired: -1, ..ChannelStats::default() },
            ..State::default()
        }
    }

    /// `_hdcd_control`: (peak_extend, target_gain)
    fn control(&self) -> (bool, i32) {
        (self.control & 16 != 0 || self.decoder_options & FLAG_FORCE_PE != 0, ((self.control & 15) as i32) << 7)
    }
}

#[derive(Clone, Debug, Default)]
struct StereoState {
    channel: [State; 2],
    ana_mode: AnalyzeMode,
    val_target_gain: i32,
    count_tg_mismatch: i32,
}

impl StereoState {
    /// `_hdcd_reset_stereo`
    fn reset(rate: u32, bits: u32, sustain_period_ms: i32, flags: i32) -> Self {
        StereoState {
            channel: [
                State::reset(rate, bits, sustain_period_ms, flags),
                State::reset(rate, bits, sustain_period_ms, flags),
            ],
            ..StereoState::default()
        }
    }

    fn set_analyze_mode(&mut self, mode: AnalyzeMode) {
        self.ana_mode = mode;
        self.channel[0].ana_mode = mode;
        self.channel[1].ana_mode = mode;
    }

    /// `_hdcd_control_stereo`: (matched, peak_extend0, peak_extend1)
    fn control(&mut self) -> (bool, bool, bool) {
        let (pe0, tg0) = self.channel[0].control();
        let (pe1, tg1) = self.channel[1].control();
        if tg0 == tg1 {
            self.val_target_gain = tg0;
            (true, pe0, pe1)
        } else {
            (false, pe0, pe1)
        }
    }
}

/// `_hdcd_integrate_x`: reads up to `count` LSBs of each channel into its
/// window and decodes a packet where one ends. Returns the frames consumed and
/// a flag per channel where a valid packet was found.
fn integrate(states: &mut [State], flag: &mut i32, samples: &[i32], mut pos: usize, count: i32, stride: usize) -> i32 {
    let channels = states.len();
    let mut bits = [0u32; 2];
    *flag = 0;
    let stride = stride.max(channels);

    let mut result = count;
    for s in states.iter() {
        result = result.min(s.readahead as i32);
    }

    for j in (0..result).rev() {
        for b in bits.iter_mut().take(channels) {
            *b |= ((samples[pos] & 1) as u32) << j;
            pos += 1;
        }
        pos += stride - channels;
    }

    for (i, s) in states.iter_mut().enumerate() {
        s.window = (s.window << result) | bits[i] as u64;
        s.readahead = s.readahead.wrapping_sub(result as u8);

        if s.readahead == 0 {
            let wbits = (s.window ^ (s.window >> 5) ^ (s.window >> 23)) as u32;
            if s.arg != 0 {
                let mut f = false;
                if wbits & 0x0fa0_0500 == 0x0fa0_0500 {
                    // A: 8-bit code 0x7e0fa005[..]
                    if wbits & 0xc8 == 0 {
                        // [..pt gggg], gain part doubled
                        s.control = ((wbits & 255) + (wbits & 7)) as u8;
                        f = true;
                        s.stats.code_counter_a += 1;
                    } else {
                        // one of bits 3, 6, or 7 was not 0
                        s.stats.code_counter_a_almost += 1;
                    }
                } else if wbits & 0xa006_0000 == 0xa006_0000 {
                    // B: 8-bit code, 8-bit XOR check, 0x7e0fa006[....]
                    if (wbits ^ (!wbits >> 8 & 255)) & 0xffff_00ff == 0xa006_0000 {
                        s.control = (wbits >> 8 & 255) as u8;
                        f = true;
                        s.stats.code_counter_b += 1;
                    } else {
                        s.stats.code_counter_b_checkfails += 1;
                    }
                }
                if f {
                    *flag |= 1 << i;
                    if s.control & 16 != 0 {
                        s.stats.count_peak_extend += 1;
                    }
                    if s.control & 32 != 0 {
                        s.stats.count_transient_filter += 1;
                    }
                    s.stats.gain_counts[(s.control & 15) as usize] += 1;
                    s.stats.max_gain = s.stats.max_gain.max((s.control & 15) as i32);
                }
                s.arg = 0;
            }
            if wbits == 0x7e0f_a005 || wbits == 0x7e0f_a006 {
                // 0x7e0fa00[.] -> [0b0101 or 0b0110]
                s.readahead = ((wbits & 3) * 8) as u8;
                s.arg = 1;
                s.stats.code_counter_c += 1;
            } else if wbits != 0 {
                s.readahead = READAHEADTAB[(wbits & 0xff) as usize];
            } else {
                s.readahead = 31; // fast forward over digital silence
            }
        }
    }
    result
}

/// `_hdcd_scan_x`: frames until the next packet (or `max`), with the code
/// detect timers.
fn scan(states: &mut [State], samples: &[i32], mut pos: usize, mut max: i32, stride: usize) -> i32 {
    let channels = states.len();
    let stride = stride.max(channels);
    let mut cdt_active = [false; 2];

    for (i, s) in states.iter_mut().enumerate() {
        if s.sustain > 0 {
            cdt_active[i] = true;
            if s.sustain <= max as u32 {
                s.control = 0;
                max = s.sustain as i32;
            }
            s.sustain = s.sustain.wrapping_sub(max as u32);
        }
    }

    let mut result = 0;
    while result < max {
        let mut flag = 0;
        let consumed = integrate(states, &mut flag, samples, pos, max - result, stride);
        result += consumed;
        if flag != 0 {
            // reset the timer where a code was detected
            for (i, s) in states.iter_mut().enumerate() {
                if flag & (1 << i) != 0 {
                    s.sustain = s.sustain_reset;
                    // first reset: from never set to never expired
                    if s.stats.count_sustain_expired == -1 {
                        s.stats.count_sustain_expired = 0;
                    }
                }
            }
            break;
        }
        pos += consumed as usize * stride;
    }

    for (i, s) in states.iter_mut().enumerate() {
        if cdt_active[i] && s.sustain == 0 {
            s.stats.count_sustain_expired += 1;
        }
    }
    result
}

/// `_hdcd_tone16`
fn tone16(sn: &mut i32, rate: i32) -> i32 {
    *sn += 1;
    match rate {
        192000 => {
            *sn %= 640;
            TONE_TAB_192000[*sn as usize] as i32
        }
        96000 => {
            *sn %= 320;
            TONE_TAB_192000[(*sn << 1) as usize] as i32
        }
        48000 => {
            *sn %= 160;
            TONE_TAB_192000[(*sn << 2) as usize] as i32
        }
        176400 => {
            *sn %= 588;
            TONE_TAB_176400[*sn as usize] as i32
        }
        88200 => {
            *sn %= 294;
            TONE_TAB_176400[(*sn << 1) as usize] as i32
        }
        _ => {
            *sn %= 147;
            TONE_TAB_176400[(*sn << 2) as usize] as i32
        }
    }
}

/// `_hdcd_analyze_prepare`: replaces the audio with a solid tone, keeping the
/// LSBs (bit 0: the HDCD signal; bit 1: the sample was above PE level).
fn analyze_prepare(state: &mut State, samples: &mut [i32], pos: usize, count: usize, stride: usize) {
    for n in (0..count * stride).step_by(stride) {
        let x = samples[pos + n];
        let mut save = if x.wrapping_abs().wrapping_sub(PEAK_EXT_LEVEL) >= 0 { 2 } else { 0 };
        save |= x & 1;
        let t = tone16(&mut state.ana_snb, state.rate);
        samples[pos + n] = (t | 3) ^ (!save & 3);
    }
}

/// `_hdcd_analyze_gen`: encodes a value in a sample's amplitude.
fn analyze_gen(sample: i32, v: u32, maxv: u32) -> i32 {
    const R: u32 = 18;
    const M: u32 = 1024;
    let v = M.wrapping_add(v.wrapping_mul(R).wrapping_mul(M) / maxv);
    ((sample as i64 * v as i64) / M as i64) as i32
}

/// `_hdcd_analyze`: like [`envelope`], but encodes the processing audibly.
#[allow(clippy::too_many_arguments)]
fn analyze(
    samples: &mut [i32],
    mut pos: usize,
    mut count: i32,
    stride: usize,
    mut gain: i32,
    target_gain: i32,
    extend: bool,
    mode: AnalyzeMode,
    cdt_active: bool,
    tg_mismatch: i32,
) -> i32 {
    const MAXG: u32 = 0xf << 7;
    let limit = count.max(0) as usize * stride;
    for i in (0..limit).step_by(stride) {
        let s = &mut samples[pos + i];
        *s = s.wrapping_shl(15);
        if mode == AnalyzeMode::Pe {
            let pel = (*s >> 16) & 1 != 0;
            *s = analyze_gen(*s, (pel && extend) as u32, 1);
        } else if (mode == AnalyzeMode::Tgm && tg_mismatch > 0) || (mode == AnalyzeMode::Cdt && cdt_active) {
            *s = analyze_gen(*s, 1, 1);
        }
    }

    if gain <= target_gain {
        let len = count.min(target_gain - gain);
        // attenuate slowly
        for _ in 0..len {
            gain += 1;
            if mode == AnalyzeMode::Lle {
                samples[pos] = analyze_gen(samples[pos], gain as u32, MAXG);
            }
            pos += stride;
        }
        count -= len.max(0);
    } else {
        let len = count.min((gain - target_gain) >> 3);
        // amplify quickly
        for _ in 0..len {
            gain -= 8;
            if mode == AnalyzeMode::Lle {
                samples[pos] = analyze_gen(samples[pos], gain as u32, MAXG);
            }
            pos += stride;
        }
        if gain - 8 < target_gain {
            gain = target_gain;
        }
        count -= len.max(0);
    }

    // hold a steady level
    if gain != 0 {
        while count > 0 {
            count -= 1;
            if mode == AnalyzeMode::Lle {
                samples[pos] = analyze_gen(samples[pos], gain as u32, MAXG);
            }
            pos += stride;
        }
    }
    gain
}

/// `APPLY_GAIN`: 11-bit (3.8) fixed point, always negative but stored positive.
#[inline]
fn apply_gain(s: &mut i32, g: i32) {
    *s = ((*s as i64 * GAINTAB[g as usize] as i64) >> 23) as i32;
}

/// `_hdcd_envelope`: applies peak extend and the gain to a run of samples.
#[allow(clippy::too_many_arguments)]
fn envelope(
    samples: &mut [i32],
    mut pos: usize,
    mut count: i32,
    stride: usize,
    bits: i32,
    mut gain: i32,
    target_gain: i32,
    extend: bool,
) -> i32 {
    let limit = count.max(0) as usize * stride;
    let (pe_level, shft) = if bits != 16 {
        ((1 << (bits - 1)) - (0x8000 - PEAK_EXT_LEVEL), (32 - bits - 1) as u32)
    } else {
        (PEAK_EXT_LEVEL, 15)
    };

    if extend {
        for i in (0..limit).step_by(stride) {
            let sample = samples[pos + i];
            let mut asample = sample.wrapping_abs().wrapping_sub(pe_level);
            samples[pos + i] = if asample >= 0 {
                if asample > PE_MAX_ASAMPLE {
                    asample = PE_MAX_ASAMPLE;
                }
                let p = PEAKTAB[asample as usize] as i32;
                if sample >= 0 { p } else { p.wrapping_neg() }
            } else {
                sample.wrapping_shl(shft)
            };
        }
    } else {
        for i in (0..limit).step_by(stride) {
            samples[pos + i] = samples[pos + i].wrapping_shl(shft);
        }
    }

    if gain <= target_gain {
        let len = count.min(target_gain - gain);
        // attenuate slowly
        for _ in 0..len {
            gain += 1;
            apply_gain(&mut samples[pos], gain);
            pos += stride;
        }
        count -= len.max(0);
    } else {
        let len = count.min((gain - target_gain) >> 3);
        // amplify quickly
        for _ in 0..len {
            gain -= 8;
            apply_gain(&mut samples[pos], gain);
            pos += stride;
        }
        if gain - 8 < target_gain {
            gain = target_gain;
        }
        count -= len.max(0);
    }

    // hold a steady level
    if gain != 0 {
        while count > 0 {
            count -= 1;
            apply_gain(&mut samples[pos], gain);
            pos += stride;
        }
    }
    gain
}

/// `_hdcd_process`: one channel at `offset`, every `stride` samples.
fn process_channel(state: &mut State, samples: &mut [i32], offset: usize, mut count: i32, stride: usize) {
    let full_count = count;
    let mut gain = state.running_gain;
    let mut lead = 0;
    let mut pos = offset;

    if state.ana_mode != AnalyzeMode::Off {
        analyze_prepare(state, samples, pos, count as usize, stride);
    }

    let (mut peak_extend, mut target_gain) = state.control();
    while count > lead {
        let run = scan(std::slice::from_mut(state), samples, pos + lead as usize * stride, count - lead, stride) + lead;
        let envelope_run = run - 1;

        gain = if state.ana_mode != AnalyzeMode::Off {
            analyze(
                samples,
                pos,
                envelope_run,
                stride,
                gain,
                target_gain,
                peak_extend,
                state.ana_mode,
                state.sustain != 0,
                -1,
            )
        } else {
            envelope(samples, pos, envelope_run, stride, state.bits, gain, target_gain, peak_extend)
        };

        pos = (pos as isize + envelope_run as isize * stride as isize) as usize;
        count -= envelope_run;
        lead = run - envelope_run;
        (peak_extend, target_gain) = state.control();
    }
    if lead > 0 {
        gain = if state.ana_mode != AnalyzeMode::Off {
            analyze(samples, pos, lead, stride, gain, target_gain, peak_extend, state.ana_mode, state.sustain != 0, -1)
        } else {
            envelope(samples, pos, lead, stride, state.bits, gain, target_gain, peak_extend)
        };
    }

    state.running_gain = gain;
    state.sample_count = state.sample_count.wrapping_add(full_count);
}

/// `_hdcd_process_stereo`: both channels together, with target gain matching.
fn process_stereo(state: &mut StereoState, samples: &mut [i32], mut count: i32) {
    const STRIDE: usize = 2;
    let full_count = count;
    let mut gain = [state.channel[0].running_gain, state.channel[1].running_gain];
    let mut lead = 0;
    let mut pos = 0usize;

    if state.ana_mode != AnalyzeMode::Off {
        analyze_prepare(&mut state.channel[0], samples, 0, count as usize, STRIDE);
        analyze_prepare(&mut state.channel[1], samples, 1, count as usize, STRIDE);
    }

    let (mut matched, mut pe0, mut pe1) = state.control();
    while count > lead {
        let run = scan(&mut state.channel, samples, pos + lead as usize * STRIDE, count - lead, 0) + lead;
        let envelope_run = run - 1;

        if !matched {
            state.count_tg_mismatch = state.count_tg_mismatch.wrapping_add(envelope_run);
        }
        run_both(state, samples, pos, envelope_run, &mut gain, pe0, pe1, matched);

        pos = (pos as isize + envelope_run as isize * STRIDE as isize) as usize;
        count -= envelope_run;
        lead = run - envelope_run;
        (matched, pe0, pe1) = state.control();
    }
    if lead > 0 {
        if !matched {
            state.count_tg_mismatch = state.count_tg_mismatch.wrapping_add(lead);
        }
        run_both(state, samples, pos, lead, &mut gain, pe0, pe1, matched);
    }

    state.channel[0].running_gain = gain[0];
    state.channel[1].running_gain = gain[1];
    state.channel[0].sample_count = state.channel[0].sample_count.wrapping_add(full_count);
    state.channel[1].sample_count = state.channel[1].sample_count.wrapping_add(full_count);
}

#[allow(clippy::too_many_arguments)]
fn run_both(
    state: &StereoState,
    samples: &mut [i32],
    pos: usize,
    n: i32,
    gain: &mut [i32; 2],
    pe0: bool,
    pe1: bool,
    matched: bool,
) {
    let tg = state.val_target_gain;
    if state.ana_mode != AnalyzeMode::Off {
        let mismatch = (!matched) as i32;
        gain[0] =
            analyze(samples, pos, n, 2, gain[0], tg, pe0, state.ana_mode, state.channel[0].sustain != 0, mismatch);
        gain[1] =
            analyze(samples, pos + 1, n, 2, gain[1], tg, pe1, state.ana_mode, state.channel[1].sustain != 0, mismatch);
    } else {
        gain[0] = envelope(samples, pos, n, 2, state.channel[0].bits, gain[0], tg, pe0);
        gain[1] = envelope(samples, pos + 1, n, 2, state.channel[1].bits, gain[1], tg, pe1);
    }
}

// --- detection (hdcd_detection_data) -----------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct DetectData {
    detected: Detected,
    packet_type: u8,
    total_packets: i32,
    errors: i32,
    peak_extend: PeakExtend,
    uses_transient_filter: bool,
    max_gain_adjustment: f32,
    cdt_expirations: i32,
    active_count: i32,
}

impl DetectData {
    fn reset() -> Self {
        DetectData { cdt_expirations: -1, ..DetectData::default() }
    }

    /// `_hdcd_detect_stereo`: `_start`, `_onech` × 2, `_end`.
    fn stereo(&mut self, state: &StereoState) {
        self.errors = 0;
        self.total_packets = 0;
        self.active_count = 0;
        self.cdt_expirations = -1;
        for s in &state.channel {
            self.one_channel(s);
        }
        if self.active_count == 2 {
            self.detected = if self.max_gain_adjustment != 0.0 || self.peak_extend != PeakExtend::Never {
                Detected::Effectual
            } else {
                Detected::NoEffect
            };
        }
    }

    fn one_channel(&mut self, state: &State) {
        let st = &state.stats;
        self.uses_transient_filter |= st.count_transient_filter != 0;
        let valid = st.code_counter_a.wrapping_add(st.code_counter_b);
        self.total_packets = self.total_packets.wrapping_add(valid);
        if st.code_counter_a != 0 {
            self.packet_type |= PacketFormat::A as u8;
        }
        if st.code_counter_b != 0 {
            self.packet_type |= PacketFormat::B as u8;
        }
        if st.count_peak_extend != 0 {
            // every valid packet used PE: permanent
            let pe = if st.count_peak_extend == valid { PeakExtend::Permanent } else { PeakExtend::Intermittent };
            if self.peak_extend != PeakExtend::Intermittent {
                self.peak_extend = pe;
            }
        }
        self.max_gain_adjustment = self.max_gain_adjustment.min(gain_to_float(st.max_gain));
        self.errors = self
            .errors
            .wrapping_add(st.code_counter_a_almost)
            .wrapping_add(st.code_counter_b_checkfails)
            .wrapping_add(st.code_counter_c_unmatched);
        if state.sustain != 0 {
            self.active_count += 1;
        }
        if st.count_sustain_expired >= 0 {
            if self.cdt_expirations == -1 {
                self.cdt_expirations = 0;
            }
            self.cdt_expirations = self.cdt_expirations.wrapping_add(st.count_sustain_expired);
        }
    }
}

/// `GAINTOFLOAT`: 4-bit (3.1) fixed point to dB (negative).
fn gain_to_float(g: i32) -> f32 {
    if g != 0 { -((g >> 1) as f32) - if g & 1 != 0 { 0.5 } else { 0.0 } } else { 0.0 }
}

// --- the simple API (hdcd_simple.c) -------------------------------------------------

/// An HDCD decoder for interleaved stereo (`hdcd_simple`).
#[derive(Clone, Debug)]
pub struct Hdcd {
    state: StereoState,
    detect: DetectData,
    stereo_mode: bool,
    rate: u32,
    bits: u32,
}

impl Default for Hdcd {
    fn default() -> Self {
        Self::new()
    }
}

impl Hdcd {
    /// `hdcd_new`: 44.1 kHz, 16-bit.
    pub fn new() -> Self {
        let mut h = Hdcd {
            state: StereoState::default(),
            detect: DetectData::reset(),
            stereo_mode: false,
            rate: 44100,
            bits: 16,
        };
        h.reset();
        h
    }

    /// `hdcd_reset`: `reset_ext(44100, 16)`.
    pub fn reset(&mut self) {
        self.reset_ext(0, 0);
    }

    /// `hdcd_reset_ext`: a new track or a seek. `rate` is 44100, 48000, 88200,
    /// 96000, 176400 or 192000 (0: 44100); `bits` is 16, 20 or 24 (0: 16).
    /// Anything else returns false and leaves the decoder as it was.
    pub fn reset_ext(&mut self, rate: u32, bits: u32) -> bool {
        let rate = match rate {
            0 => 44100,
            44100 | 88200 | 176400 | 48000 | 96000 | 192000 => rate,
            _ => return false,
        };
        let bits = match bits {
            0 => 16,
            16 | 20 | 24 => bits,
            _ => return false,
        };
        self.rate = rate;
        self.bits = bits;
        self.state = StereoState::reset(rate, bits, 0, FLAG_TGM_LOG_OFF);
        self.detect = DetectData::reset();
        self.set_analyze_mode(AnalyzeMode::Off);
        self.stereo_mode = true;
        true
    }

    /// `hdcd_process`: interleaved stereo samples with the LSB in bit 0
    /// (16-bit values for 16-bit sources), decoded in place. 16-bit input
    /// comes out as 32-bit samples at -6 dB. A trailing odd sample is left alone.
    pub fn process(&mut self, samples: &mut [i32]) {
        let count = (samples.len() / 2).min(i32::MAX as usize) as i32;
        if self.stereo_mode {
            process_stereo(&mut self.state, samples, count);
        } else {
            let [c0, c1] = &mut self.state.channel;
            process_channel(c0, samples, 0, count, 2);
            process_channel(c1, samples, 1, count, 2);
        }
        self.detect.stereo(&self.state);
    }

    /// Sonkore's path (the engine): the decoder's left-justified s32 samples of
    /// a 16-bit source, shifted down to 16 bits and decoded in place.
    pub fn process_s32(&mut self, samples: &mut [i32]) {
        for s in samples.iter_mut() {
            *s >>= 16;
        }
        self.process(samples);
    }

    /// `hdcd_scan`: what [`Hdcd::detected`] would be after processing
    /// `samples`, leaving them and the decoder untouched. With `ignore_state`,
    /// as if from a reset.
    pub fn scan(&self, samples: &[i32], ignore_state: bool) -> Detected {
        let (mut st, mut d) = if ignore_state {
            (StereoState::reset(self.rate, self.bits, 0, FLAG_TGM_LOG_OFF), DetectData::reset())
        } else {
            (self.state.clone(), self.detect)
        };
        if d.detected == Detected::Effectual {
            return d.detected;
        }
        let mut copy = samples[..samples.len() / 2 * 2].to_vec();
        let count = (copy.len() / 2) as i32;
        process_stereo(&mut st, &mut copy, count);
        d.stereo(&st);
        d.detected
    }

    /// `hdcd_analyze_mode`
    pub fn set_analyze_mode(&mut self, mode: AnalyzeMode) -> bool {
        for c in &mut self.state.channel {
            c.decoder_options &= !FLAG_FORCE_PE;
        }
        match mode {
            AnalyzeMode::Off | AnalyzeMode::Lle | AnalyzeMode::Pe | AnalyzeMode::Cdt | AnalyzeMode::Tgm => {
                self.stereo_mode = true;
                self.state.set_analyze_mode(mode);
            }
            AnalyzeMode::Pel => {
                self.stereo_mode = true;
                for c in &mut self.state.channel {
                    c.decoder_options |= FLAG_FORCE_PE;
                }
                self.state.set_analyze_mode(AnalyzeMode::Pe);
            }
            AnalyzeMode::Ltgm => {
                self.stereo_mode = false;
                self.state.set_analyze_mode(AnalyzeMode::Lle);
            }
        }
        true
    }

    /// `hdcd_detected`
    pub fn detected(&self) -> Detected {
        self.detect.detected
    }

    /// Everything the `hdcd_detect_*` getters report.
    pub fn detection(&self) -> Detection {
        let d = &self.detect;
        Detection {
            detected: d.detected,
            packet_type: PacketFormat::from_bits(d.packet_type),
            total_packets: d.total_packets,
            errors: d.errors,
            peak_extend: d.peak_extend,
            uses_transient_filter: d.uses_transient_filter,
            max_gain_adjustment: d.max_gain_adjustment,
            cdt_expirations: d.cdt_expirations,
            lle_mismatch: self.state.count_tg_mismatch,
        }
    }

    /// `hdcd_detect_str`
    pub fn detect_str(&self) -> String {
        self.detection().summary()
    }

    /// A channel's counters (0 left, 1 right).
    pub fn channel_stats(&self, channel: usize) -> ChannelStats {
        self.state.channel[channel].stats
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn bits(&self) -> u32 {
        self.bits
    }
}

// --- Sonkore's detection ----------------------------------------------------------

/// How much of a track is looked at for HDCD codes.
pub const DETECT_SECONDS: u64 = 5;
/// Frames per read while detecting.
pub const DETECT_BLOCK_FRAMES: usize = 4096;

/// Only 16-bit stereo PCM tracks can be HDCD (DSD never).
pub fn eligible(channels: usize, bits: u32) -> bool {
    channels == 2 && bits == 16
}

/// Detection once the stream is known to be [`eligible`]: do
/// the first 5 s carry HDCD codes? `read` fills its buffer with interleaved
/// stereo frames as left-justified s32 (16-bit << 16), like the decoder's
/// `read_i32`, and returns the frames read (0 at the end).
pub fn detect<F>(rate: u32, mut read: F) -> bool
where
    F: FnMut(&mut [i32]) -> usize,
{
    let mut ctx = Hdcd::new();
    if !ctx.reset_ext(rate, 16) {
        return false;
    }
    let mut buf = vec![0i32; DETECT_BLOCK_FRAMES * 2];
    let limit = rate as u64 * DETECT_SECONDS;
    let mut done = 0u64;
    let mut found = false;
    while done < limit && !found {
        let n = read(&mut buf).min(DETECT_BLOCK_FRAMES);
        if n == 0 {
            break;
        }
        ctx.process_s32(&mut buf[..n * 2]);
        found = ctx.detected() != Detected::None;
        done += n as u64;
    }
    found
}
