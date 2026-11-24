//! The player (transport and queue): plays the queue on the
//! engine, keeps the next track armed for gapless joins, moves on when a
//! track ends, skips files that do not open.

use std::path::Path;

use crate::engine::{Engine, ErrorKind, State};
use crate::queue::{Queue, Repeat};

/// What changed in a call, for the interface to refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub track: bool,
    pub state: bool,
    pub queue: bool,
    pub error: bool,
    /// The queue ran out (nothing repeats).
    pub finished: bool,
    /// The device stopped for good (unplugged, the sound server gone): it
    /// was closed, and the track opens again where it was on the next play.
    pub output_lost: bool,
}

impl Changes {
    pub fn merge(&mut self, o: Changes) {
        self.track |= o.track;
        self.state |= o.state;
        self.queue |= o.queue;
        self.error |= o.error;
        self.finished |= o.finished;
        self.output_lost |= o.output_lost;
    }
}

pub struct Player {
    engine: Engine,
    queue: Queue,
    armed: Option<String>,
    loaded: bool,
    resume_position: f64,
    last_error: String,
    /// What failed the last time a track did not open.
    last_error_kind: Option<ErrorKind>,
    /// Each track's ReplayGain (linear), known before it plays: set before a
    /// track opens and given with the next one, so it lands on its first sample.
    gain_of: Option<GainSource>,
}

/// A track's ReplayGain from its path; None to leave the gain as it is.
pub type GainSource = Box<dyn Fn(&str) -> Option<f64>>;

impl Player {
    pub fn new(engine: Engine) -> Self {
        Player {
            engine,
            queue: Queue::new(),
            armed: None,
            loaded: false,
            resume_position: 0.0,
            last_error: String::new(),
            last_error_kind: None,
            gain_of: None,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }
    pub fn queue(&self) -> &Queue {
        &self.queue
    }
    pub fn loaded(&self) -> bool {
        self.loaded
    }
    pub fn playing(&self) -> bool {
        self.engine.state() == State::Playing
    }
    pub fn last_error(&self) -> &str {
        &self.last_error
    }
    pub fn current_path(&self) -> Option<&str> {
        self.queue.current_path()
    }

    /// Seconds into the current track (or where a restored session resumes).
    pub fn position(&self) -> f64 {
        if self.loaded { self.engine.position() } else { self.resume_position }
    }

    /// A queue from the last session: shown, opened on the first play.
    pub fn restore(&mut self, paths: Vec<String>, index: usize, position: f64) {
        self.queue.set(paths, index);
        self.resume_position = position.max(0.0);
    }

    pub fn play_paths(&mut self, paths: Vec<String>, start: usize) -> Changes {
        if paths.is_empty() {
            return Changes::default();
        }
        self.queue.set(paths, start);
        let mut c = Changes { queue: true, ..Default::default() };
        c.merge(self.open_index(self.queue.current().unwrap_or(0), true, 0.0));
        c
    }

    pub fn enqueue(&mut self, paths: Vec<String>) -> Changes {
        let mut c = Changes { queue: !paths.is_empty(), ..Default::default() };
        if self.queue.enqueue(paths) {
            c.merge(self.open_index(0, true, 0.0));
        } else {
            self.arm_gapless();
        }
        c
    }

    /// Another output: the track opens on it again where it was, playing on
    /// if it played.
    pub fn set_device(&mut self, key: &str) -> Changes {
        if key == self.engine.device_key() {
            return Changes::default();
        }
        let current = self.queue.current().filter(|_| self.loaded);
        let (playing, at) = (self.playing(), self.engine.position());
        self.engine.set_device(key);
        match current {
            Some(index) => self.open_index(index, playing, at),
            None => Changes::default(),
        }
    }

    pub fn play_index(&mut self, index: usize) -> Changes {
        if index >= self.queue.len() {
            return Changes::default();
        }
        self.open_index(index, true, 0.0)
    }

    pub fn play(&mut self) -> Changes {
        if !self.loaded {
            // The restored queue opens lazily on the first play.
            return match self.queue.current() {
                Some(i) => self.open_index(i, true, self.resume_position),
                None => Changes::default(),
            };
        }
        self.engine.play();
        Changes { state: true, ..Default::default() }
    }

    /// The output was closed by a setting that changes how it opens (the
    /// rate mode, PipeWire following the track's rate, the buffer…): the
    /// track opens again at `at`, playing on if `playing`.
    pub fn reopen(&mut self, playing: bool, at: f64) -> Changes {
        match self.queue.current().filter(|_| self.loaded) {
            Some(index) => self.open_index(index, playing, at),
            None => Changes::default(),
        }
    }

    /// The device closed (a DAC given back to the system): the track stays
    /// where it was and opens again, there, on the next play.
    pub fn release_output(&mut self) -> Changes {
        if self.loaded {
            self.resume_position = self.engine.position();
        }
        self.engine.pause();
        self.engine.close_output();
        self.loaded = false;
        self.armed = None;
        Changes { state: true, ..Default::default() }
    }

    pub fn pause(&mut self) -> Changes {
        self.engine.pause();
        Changes { state: true, ..Default::default() }
    }

    pub fn toggle(&mut self) -> Changes {
        if self.playing() { self.pause() } else { self.play() }
    }

    pub fn stop(&mut self) -> Changes {
        self.engine.stop();
        self.loaded = false;
        self.armed = None;
        self.resume_position = 0.0;
        Changes { state: true, ..Default::default() }
    }

    pub fn seek(&mut self, seconds: f64) {
        if self.loaded {
            let _ = self.engine.seek(seconds);
        } else {
            self.resume_position = seconds.max(0.0);
        }
    }

    pub fn next(&mut self) -> Changes {
        self.advance(true)
    }

    /// Past 3 s the track starts again; else the one before (wrapping under
    /// repeat all).
    pub fn previous(&mut self) -> Changes {
        if self.queue.is_empty() {
            return Changes::default();
        }
        if self.loaded && self.engine.position() > 3.0 {
            let _ = self.engine.seek(0.0);
            return Changes::default();
        }
        match self.queue.previous_index() {
            Some(i) => {
                let autoplay = self.playing() || !self.loaded;
                self.open_index(i, autoplay, 0.0)
            }
            None => Changes::default(),
        }
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.queue.set_shuffle(on);
        self.arm_gapless();
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.queue.set_repeat(repeat);
        self.arm_gapless();
    }

    pub fn remove(&mut self, indices: &[usize]) -> Changes {
        let changed = self.queue.remove(indices);
        self.arm_gapless();
        Changes { queue: changed, track: changed, ..Default::default() }
    }

    pub fn move_entries(&mut self, indices: &[usize], target: usize) -> Changes {
        let changed = self.queue.move_entries(indices, target);
        self.arm_gapless();
        Changes { queue: changed, track: changed, ..Default::default() }
    }

    pub fn reverse(&mut self) -> Changes {
        let changed = self.queue.reverse();
        self.arm_gapless();
        Changes { queue: changed, track: changed, ..Default::default() }
    }

    /// Puts the queue in `order` (old indices), as sortQueue does.
    pub fn apply_order(&mut self, order: &[usize]) -> Changes {
        let changed = self.queue.apply_order(order);
        self.arm_gapless();
        Changes { queue: changed, track: changed, ..Default::default() }
    }

    pub fn clear(&mut self) -> Changes {
        self.engine.stop();
        self.queue.clear();
        self.loaded = false;
        self.armed = None;
        Changes { queue: true, track: true, state: true, ..Default::default() }
    }

    /// A library folder removed: its tracks leave the queue (and the player,
    /// when one of them is playing).
    pub fn forget_folder(&mut self, folder: &str) -> Changes {
        if self.queue.forget_folder(folder) {
            self.engine.stop();
            self.loaded = false;
            self.armed = None;
            self.resume_position = 0.0;
            return Changes { queue: true, track: true, state: true, ..Default::default() };
        }
        self.arm_gapless();
        Changes { queue: true, track: true, ..Default::default() }
    }

    /// Called on a timer (every 50 ms while playing): joins heard, tracks
    /// ended.
    pub fn poll(&mut self) -> Changes {
        if self.loaded && self.engine.output_failed() {
            return self.output_lost();
        }
        let mut c = Changes::default();
        if self.engine.take_track_change() {
            // The next track joined without a gap and is being heard.
            let path = self.engine.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let upcoming = self.queue.upcoming();
            let index = match upcoming {
                Some(u) if self.queue.paths()[u] == path => Some(u),
                _ => self.queue.paths().iter().position(|p| *p == path),
            };
            if index.is_some() {
                self.queue.set_current(index);
            }
            self.armed = None;
            self.arm_gapless();
            c.track = true;
        }
        if self.playing() && self.engine.finished() {
            c.merge(self.advance(false));
        }
        c
    }

    fn advance(&mut self, user: bool) -> Changes {
        if self.queue.is_empty() {
            return Changes::default();
        }
        if !user && self.queue.repeat() == Repeat::One {
            let _ = self.engine.seek(0.0);
            self.engine.play();
            return Changes { state: true, ..Default::default() };
        }
        // Unreadable files are skipped, never in a loop.
        let candidates = self.queue.next_candidates(user);
        if candidates.is_empty() {
            self.engine.stop();
            self.loaded = false;
            self.armed = None;
            return Changes { state: true, finished: true, ..Default::default() };
        }
        let mut c = Changes::default();
        for i in candidates {
            let opened = self.open_index(i, true, 0.0);
            c.merge(opened);
            // A device that failed fails every file alike: no use trying
            // the whole queue (each with its retries).
            if self.loaded || self.last_error_kind == Some(ErrorKind::Output) {
                return c;
            }
        }
        c
    }

    /// The device stopped for good: it is closed (nothing waits on it any
    /// more) and the track stays where it was, to open again on play.
    fn output_lost(&mut self) -> Changes {
        let at = self.engine.position();
        eprintln!("sonkore: the output \"{}\" stopped", self.engine.device_key());
        self.engine.pause();
        self.engine.close_output();
        self.loaded = false;
        self.armed = None;
        self.resume_position = at;
        self.last_error = "The audio output stopped".into();
        self.last_error_kind = Some(ErrorKind::Output);
        Changes { state: true, error: true, output_lost: true, ..Default::default() }
    }

    /// Where each track's ReplayGain comes from (see `GainSource`).
    pub fn set_gain_source(&mut self, f: GainSource) {
        self.gain_of = Some(f);
    }

    /// The ReplayGain settings changed: the track armed for the join gets
    /// its new gain too.
    pub fn refresh_next_gain(&mut self) {
        let gain = self.armed.as_deref().and_then(|p| self.gain_of.as_ref().and_then(|f| f(p)));
        self.engine.set_next_track_gain(gain);
    }

    fn open_index(&mut self, index: usize, autoplay: bool, start: f64) -> Changes {
        let Some(path) = self.queue.paths().get(index).cloned() else { return Changes::default() };
        self.queue.set_current(Some(index));
        if let Some(gain) = self.gain_of.as_ref().and_then(|f| f(&path)) {
            self.engine.set_track_gain(gain);
        }
        let mut c = Changes { track: true, state: true, ..Default::default() };
        match self.engine.load(Path::new(&path), start) {
            Ok(_) => {
                self.loaded = true;
                if autoplay {
                    self.engine.play();
                }
                if !self.last_error.is_empty() {
                    self.last_error.clear();
                    c.error = true;
                }
                self.last_error_kind = None;
            }
            Err(e) => {
                self.loaded = false;
                // The cause as it came, for a terminal (the message shown is friendlier).
                eprintln!("sonkore: cannot play {path} on \"{}\": {}", self.engine.device_key(), e.0);
                self.last_error = load_error(&path, &e.0);
                self.last_error_kind = Some(e.kind());
                c.error = true;
            }
        }
        self.resume_position = 0.0;
        self.armed = None;
        self.arm_gapless();
        c
    }

    /// The track that follows, opened ahead for a gapless join.
    fn arm_gapless(&mut self) {
        let next = if self.loaded { self.queue.upcoming() } else { None };
        let path = next.and_then(|i| self.queue.paths().get(i).cloned());
        if path == self.armed {
            return;
        }
        self.engine.set_next(path.as_deref().map(Path::new));
        let gain = path.as_deref().and_then(|p| self.gain_of.as_ref().and_then(|f| f(p)));
        self.engine.set_next_track_gain(gain);
        self.armed = path;
    }
}

/// Why a track did not open, in words that say what to do.
fn load_error(path: &str, error: &str) -> String {
    let file = Path::new(path);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if file.exists() {
        if std::fs::File::open(file).is_err() {
            return format!("No permission to read {name}");
        }
        return error.to_owned();
    }
    format!("File not found: {name}")
}
