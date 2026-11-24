//! The interleaved FIFO that links the stages (FIFOSampleBuffer).
//!
//! Counts are in frames. The buffer keeps its high-water capacity, so once a
//! stream has run for a moment at given settings nothing allocates any more.

pub(crate) struct Fifo {
    buf: Vec<f32>, // always fully initialised: len() == capacity in samples
    channels: usize,
    pos: usize, // first unread frame
    len: usize, // frames held
}

impl Fifo {
    pub fn new(channels: usize) -> Self {
        Fifo { buf: Vec::new(), channels, pos: 0, len: 0 }
    }

    /// Room for `frames` frames without growing.
    pub fn reserve(&mut self, frames: usize) {
        self.make_room(frames.saturating_sub(self.len));
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// The frames held, interleaved.
    pub fn data(&self) -> &[f32] {
        &self.buf[self.pos * self.channels..(self.pos + self.len) * self.channels]
    }

    /// Free space for at least `frames` frames past the end (ptrEnd), to be
    /// committed with [`Fifo::commit`].
    pub fn tail(&mut self, frames: usize) -> &mut [f32] {
        self.make_room(frames);
        let start = (self.pos + self.len) * self.channels;
        &mut self.buf[start..start + frames * self.channels]
    }

    /// Counts `frames` frames written into [`Fifo::tail`] as held.
    pub fn commit(&mut self, frames: usize) {
        debug_assert!((self.pos + self.len + frames) * self.channels <= self.buf.len());
        self.len += frames;
    }

    pub fn put(&mut self, samples: &[f32]) {
        let frames = samples.len() / self.channels;
        let samples = &samples[..frames * self.channels];
        self.tail(frames).copy_from_slice(samples);
        self.len += frames;
    }

    pub fn add_silence(&mut self, frames: usize) {
        self.tail(frames).fill(0.0);
        self.len += frames;
    }

    /// Copies out up to `out.len() / channels` frames and drops them.
    pub fn receive_into(&mut self, out: &mut [f32]) -> usize {
        let n = (out.len() / self.channels).min(self.len);
        out[..n * self.channels].copy_from_slice(&self.data()[..n * self.channels]);
        self.skip(n)
    }

    /// Drops up to `frames` frames from the front.
    pub fn skip(&mut self, frames: usize) -> usize {
        if frames >= self.len {
            let n = self.len;
            self.len = 0;
            self.pos = 0;
            return n;
        }
        self.len -= frames;
        self.pos += frames;
        frames
    }

    /// Trims the content down to `frames` frames (adjustAmountOfSamples).
    pub fn truncate(&mut self, frames: usize) -> usize {
        self.len = self.len.min(frames);
        self.len
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.pos = 0;
    }

    /// Makes `frames` free frames past the end: moves the content to the
    /// front, and grows (geometrically) only when that is not enough.
    fn make_room(&mut self, frames: usize) {
        let ch = self.channels;
        let need = (self.len + frames) * ch;
        if (self.pos + self.len + frames) * ch <= self.buf.len() {
            return;
        }
        if self.pos > 0 {
            self.buf.copy_within(self.pos * ch..(self.pos + self.len) * ch, 0);
            self.pos = 0;
        }
        if need > self.buf.len() {
            let grown = need.max(self.buf.len() * 2).max(1024 * ch);
            self.buf.resize(grown, 0.0);
        }
    }
}
