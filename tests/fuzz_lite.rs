//! Fuzz-lite: deterministic mutation tests on stable Rust, shared by the
//! crates' `tests/robustness.rs` (`#[path = "../../../tests/fuzz_lite.rs"]`).
//!
//! Seeds (corpus files from `target/test-corpus`, `target/library-corpus`,
//! `target/dsd-corpus`, skipped when absent, plus the synthetic headers each
//! test builds) are mutated with a fixed-seed xorshift: bit flips, bytes,
//! size fields set to 0 / u32::MAX / u64::MAX-ish (next to the four-letter
//! chunk ids), truncation, chunk duplication, deletion, insertion. Each case
//! runs inside `catch_unwind` on a thread with a small stack (deep recursion
//! overflows it), watched by the test's thread (a case that never ends fails
//! after a time bound), with a global allocator that notes the largest single
//! allocation (a size taken from a header: gigabytes for a 60 KB file).
//!
//! A failing case is written to `target/fuzz-lite/<target>-<case>.<ext>`.
//! - `SONKORE_FUZZ_CASE=<target>:<n>` runs only case n of that target.
//! - `SONKORE_FUZZ_SCALE=10` runs ten times as many cases (a longer hunt).
//! - `SONKORE_FUZZ_TRACE=1` prints each case before it runs: a stack
//!   overflow aborts the whole binary, and the last line names the case.

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

// --- Random numbers ------------------------------------------------------------------

/// xorshift64*: tiny, fast, and the same everywhere.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        r.next();
        r
    }

    /// The generator of case `case` of `target`: each case reproduces alone.
    pub fn for_case(target: &str, case: usize) -> Rng {
        let mut h = 0xCBF2_9CE4_8422_2325u64; // FNV-1a
        for b in target.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01B3);
        }
        Rng::new(h ^ (case as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93))
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0..n (0 when n is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

// --- The allocator ---------------------------------------------------------------------

/// Larger single allocations than this, made while a case runs, fail it.
pub const BIG_ALLOCATION: usize = 256 << 20;
/// Larger than this is refused outright (the process aborts with "memory
/// allocation of N bytes failed"): the machine is not to start swapping.
/// Above u32::MAX, so a 4 GiB size from a 32-bit field (mostly reserved,
/// never touched) is a failed case and not the end of the whole binary.
const REFUSED_ALLOCATION: usize = 5 << 30;

thread_local! {
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static BIGGEST: Cell<usize> = const { Cell::new(0) };
    static IN_CASE: Cell<bool> = const { Cell::new(false) };
    static LAST_PANIC: RefCell<String> = const { RefCell::new(String::new()) };
    /// The case running on this thread (for the refused-allocation note).
    static CASE: Cell<usize> = const { Cell::new(usize::MAX) };
    static TARGET: Cell<&'static str> = const { Cell::new("") };
    /// Where the biggest allocation of the case was made.
    static BIG_WHERE: RefCell<Option<std::backtrace::Backtrace>> = const { RefCell::new(None) };
}

/// The test binaries' global allocator: the system's, noting the largest
/// single allocation a case makes.
pub struct Guard;

fn note(size: usize) -> bool {
    WATCHING
        .try_with(|w| {
            if !w.get() {
                return false;
            }
            let biggest = BIGGEST.try_with(|b| b.replace(b.get().max(size))).unwrap_or(usize::MAX);
            if size > BIG_ALLOCATION && size > biggest {
                // Where from: the frames in this repository's and the
                // dependencies' code (taken unwatched: it allocates).
                w.set(false);
                // (Resolved only for the first few failures: that is slow.)
                let wanted = Some(std::backtrace::Backtrace::force_capture());
                let _ = BIG_WHERE.try_with(|b| *b.borrow_mut() = wanted);
                w.set(true);
            }
            let refused = size > REFUSED_ALLOCATION;
            if refused {
                // The process aborts next ("memory allocation of N bytes
                // failed"): say which case, without allocating.
                let mut line = StackText::default();
                let case = CASE.try_with(Cell::get).unwrap_or(usize::MAX);
                let target = TARGET.try_with(Cell::get).unwrap_or("");
                let _ = writeln!(
                    line,
                    "fuzz-lite: {target}:{case} asked for {size} bytes at once (refused); SONKORE_FUZZ_CASE={target}:{case} runs it alone"
                );
                let _ = std::io::Write::write_all(&mut std::io::stderr(), line.as_bytes());
            }
            refused
        })
        .unwrap_or(false)
}

/// The frames of `bt` in Sonkore's code and the decoders'.
fn frames_of(bt: &std::backtrace::Backtrace) -> String {
    let text = bt.to_string();
    let mut wanted = String::new();
    let mut n = 0;
    for frame in text.lines().map(str::trim).filter(|l| l.contains(": ")) {
        let name = frame.split_once(": ").map_or(frame, |x| x.1);
        if (name.contains("symphonia") || name.contains("sonkore") || name.contains("ffmpeg"))
            && !name.contains("fuzz_lite")
        {
            wanted.push_str("\n  in ");
            wanted.push_str(name);
            n += 1;
            if n == 4 {
                break;
            }
        }
    }
    wanted
}

/// Text formatted on the stack (inside the allocator nothing may allocate).
struct StackText {
    buf: [u8; 240],
    len: usize,
}

impl Default for StackText {
    fn default() -> Self {
        StackText { buf: [0; 240], len: 0 }
    }
}

impl StackText {
    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl std::fmt::Write for StackText {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

// SAFETY: forwards to the system allocator; refusing returns null, which
// GlobalAlloc allows.
unsafe impl GlobalAlloc for Guard {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if note(layout.size()) {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if note(layout.size()) {
            return std::ptr::null_mut();
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if note(new_size) {
            return std::ptr::null_mut();
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// An empty folder for `target`'s case files, in the system's temporary
/// folder (tmpfs on most Linux systems: a file per case costs no disk I/O).
/// Folders left by runs that ended (killed, aborted) are removed.
pub fn scratch(target: &str) -> PathBuf {
    let root = std::env::temp_dir().join("sonkore-fuzz-lite");
    if cfg!(target_os = "linux")
        && let Ok(runs) = std::fs::read_dir(&root)
    {
        for run in runs.flatten() {
            let pid = run.file_name().to_string_lossy().into_owned();
            if !Path::new("/proc").join(&pid).exists() {
                let _ = std::fs::remove_dir_all(run.path());
            }
        }
    }
    let d = root.join(std::process::id().to_string()).join(target);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// --- Seeds -------------------------------------------------------------------------

/// The repository (the including crate is crates/<name>).
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Clone)]
pub struct Seed {
    pub name: String,
    /// Lower case, without the dot.
    pub ext: String,
    pub bytes: Vec<u8>,
}

impl Seed {
    pub fn new(name: &str, ext: &str, bytes: Vec<u8>) -> Seed {
        Seed { name: name.into(), ext: ext.into(), bytes }
    }
}

/// Files up to this size are seeds whole; bigger ones by their first
/// `HEAD` bytes (the headers; writing megabytes per case is too slow).
const WHOLE: usize = 160 << 10;
const HEAD: usize = 64 << 10;

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() && depth > 0 => walk(&p, depth - 1, out),
            Ok(m) if m.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// The corpus files with one of `exts` (lower case), in a fixed order,
/// identical contents once. Empty without a corpus.
pub fn corpus_seeds(exts: &[&str]) -> Vec<Seed> {
    corpus_seeds_up_to(exts, WHOLE)
}

/// `corpus_seeds`, files up to `whole` bytes kept whole (readers that take
/// a buffer instead of a path: nothing to write per case).
pub fn corpus_seeds_up_to(exts: &[&str], whole: usize) -> Vec<Seed> {
    let mut dirs: Vec<PathBuf> =
        ["test-corpus", "library-corpus", "dsd-corpus"].iter().map(|d| repo().join("target").join(d)).collect();
    if let Some(d) = std::env::var_os("SONKORE_TEST_CORPUS") {
        dirs.push(PathBuf::from(d));
    }
    let mut files = Vec::new();
    for d in &dirs {
        walk(d, 6, &mut files);
    }
    files.sort();
    let mut seen = std::collections::HashSet::new();
    let mut seeds = Vec::new();
    for f in files {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if !exts.contains(&ext.as_str()) {
            continue;
        }
        let Ok(mut bytes) = std::fs::read(&f) else { continue };
        if bytes.len() > whole {
            bytes.truncate(HEAD);
        }
        if !seen.insert(fnv(&bytes)) {
            continue;
        }
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        seeds.push(Seed { name, ext, bytes });
    }
    seeds
}

pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01B3);
    }
    h
}

// --- Mutations ---------------------------------------------------------------------

/// Values that break size arithmetic.
const INTERESTING: [u64; 19] = [
    0,
    1,
    2,
    0x7F,
    0x80,
    0xFF,
    0x7FFF,
    0x8000,
    0xFFFF,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFF0,
    0xFFFF_FFF8,
    0xFFFF_FFFF,
    0x1_0000_0000,
    i64::MAX as u64,
    u64::MAX - 11,
    u64::MAX - 7,
    u64::MAX,
];

/// Where to change something: mostly the headers.
fn position(rng: &mut Rng, len: usize) -> usize {
    match rng.below(100) {
        0..60 => rng.below(len.min(1024)),
        60..85 => rng.below(len.min(16 << 10)),
        _ => rng.below(len),
    }
}

/// A four-letter chunk id at or after `from` (RIFF, IFF, DSDIFF, MP4 atoms…).
fn fourcc_after(d: &[u8], from: usize) -> Option<usize> {
    let ok = |c: u8| c.is_ascii_alphanumeric() || c == b' ' || c == 0xA9;
    let end = d.len().min(from + 512);
    (from..end.saturating_sub(3))
        .find(|&i| d[i..i + 4].iter().all(|&c| ok(c)) && d[i..i + 4].iter().any(|&c| c != b' '))
}

fn put_int(rng: &mut Rng, d: &mut [u8], at: usize, value: u64) {
    let width = *rng.pick(&[1usize, 2, 4, 4, 4, 8, 8]);
    let bytes = if rng.chance(50) { value.to_le_bytes() } else { value.to_be_bytes() };
    let bytes: &[u8] = if rng.chance(50) { &bytes[..width] } else { &bytes[8 - width..] };
    for (i, b) in bytes.iter().enumerate() {
        if let Some(x) = d.get_mut(at + i) {
            *x = *b;
        }
    }
}

fn mutate_once(rng: &mut Rng, d: &mut Vec<u8>) {
    let len = d.len();
    if len == 0 {
        let n = 1 + rng.below(64);
        d.extend(rng.bytes(n));
        return;
    }
    match rng.below(12) {
        0..=2 => {
            let at = position(rng, len);
            d[at] ^= 1 << rng.below(8);
        }
        3 => {
            let at = position(rng, len);
            d[at] = rng.next() as u8;
        }
        4 => {
            let at = position(rng, len);
            let v = *rng.pick(&INTERESTING);
            put_int(rng, d, at, v);
        }
        5 | 6 => {
            // A size field: after the chunk id (RIFF, IFF, DSF, DSDIFF) or
            // before it (MP4), 32 or 64 bits.
            let from = position(rng, len);
            if let Some(id) = fourcc_after(d, from) {
                let v = *rng.pick(&INTERESTING);
                let at = if rng.chance(70) { id + 4 } else { id.saturating_sub(4) };
                let bytes = if rng.chance(30) { v.to_le_bytes() } else { v.to_be_bytes() };
                let wide = rng.chance(40);
                let bytes: &[u8] = if wide {
                    &bytes
                } else if bytes[0] == (v >> 56) as u8 {
                    &bytes[4..]
                } else {
                    &bytes[..4]
                };
                for (i, b) in bytes.iter().enumerate() {
                    if let Some(x) = d.get_mut(at + i) {
                        *x = *b;
                    }
                }
            } else {
                let at = position(rng, len);
                d[at] ^= 0xFF;
            }
        }
        7 => {
            let at = position(rng, len);
            d.truncate(at);
        }
        8 => {
            // A chunk twice (or a piece of one somewhere else).
            let from = position(rng, len);
            let n = 1 + rng.below((len - from).min(4096));
            let piece = d[from..from + n].to_vec();
            let to = if rng.chance(50) { from + n } else { position(rng, len) };
            d.splice(to..to, piece);
        }
        9 => {
            let from = position(rng, len);
            let n = 1 + rng.below((len - from).min(256));
            d.drain(from..from + n);
        }
        10 => {
            let at = position(rng, len);
            let n = 1 + rng.below(32);
            let fill = rng.bytes(n);
            d.splice(at..at, fill);
        }
        _ => {
            let at = position(rng, len);
            let n = 1 + rng.below(64);
            let b = *rng.pick(&[0x00u8, 0xFF, 0x80, 0x7F]);
            for x in d.iter_mut().skip(at).take(n) {
                *x = b;
            }
        }
    }
}

/// One to a few mutations of `seed`.
pub fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut d = seed.to_vec();
    let mut n = 1 + rng.below(3);
    if rng.chance(10) {
        n += rng.below(10);
    }
    for _ in 0..n {
        mutate_once(rng, &mut d);
    }
    d
}

/// Every truncation of a small seed; of a bigger one, the first 256 and
/// `count` more spread over the rest (none for 0).
pub fn truncations(len: usize, count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    if len <= 256 + count {
        return (0..len).collect();
    }
    let mut v: Vec<usize> = (0..256).collect();
    v.extend((0..count).map(|i| 256 + (len - 256) * i / count));
    v
}

// --- The runner --------------------------------------------------------------------

/// One input.
pub struct Case {
    pub label: String,
    /// The file extension it is read with.
    pub ext: String,
    pub bytes: Vec<u8>,
}

/// The usual plan over seeds: each seed's truncations, then mutations
/// round-robin; `mutations` in all. Case n is always the same input.
pub struct Plan {
    seeds: Vec<Seed>,
    truncated: Vec<(usize, usize)>,
    mutations: usize,
    /// Other extensions a case is sometimes read as (format confusion).
    pub other_exts: Vec<String>,
}

impl Plan {
    pub fn new(seeds: Vec<Seed>, mutations: usize, truncations_per_seed: usize) -> Plan {
        let mut truncated = Vec::new();
        for (i, s) in seeds.iter().enumerate() {
            for t in truncations(s.bytes.len(), truncations_per_seed) {
                truncated.push((i, t));
            }
        }
        Plan { seeds, truncated, mutations: scaled(mutations), other_exts: Vec::new() }
    }

    pub fn len(&self) -> usize {
        if self.seeds.is_empty() { 0 } else { self.truncated.len() + self.mutations }
    }

    pub fn seeds(&self) -> &[Seed] {
        &self.seeds
    }

    pub fn case(&self, target: &str, n: usize) -> Case {
        if let Some(&(i, at)) = self.truncated.get(n) {
            let s = &self.seeds[i];
            return Case {
                label: format!("{} cut at {at}", s.name),
                ext: s.ext.clone(),
                bytes: s.bytes[..at].to_vec(),
            };
        }
        let mut rng = Rng::for_case(target, n);
        let s = &self.seeds[(n - self.truncated.len()) % self.seeds.len()];
        let bytes = mutate(&mut rng, &s.bytes);
        let ext = if !self.other_exts.is_empty() && rng.chance(8) {
            rng.pick(&self.other_exts).clone()
        } else {
            s.ext.clone()
        };
        Case { label: format!("{} mutated (case {n})", s.name), ext, bytes }
    }
}

/// `n` times SONKORE_FUZZ_SCALE.
pub fn scaled(n: usize) -> usize {
    let scale: f64 = std::env::var("SONKORE_FUZZ_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    ((n as f64 * scale) as usize).max(1)
}

fn install_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if IN_CASE.try_with(Cell::get).unwrap_or(false) {
                // Kept for the report instead of printed for every case.
                let _ = LAST_PANIC.try_with(|p| *p.borrow_mut() = info.to_string());
            } else {
                previous(info);
            }
        }));
    });
}

fn save(target: &str, n: usize, case: &Case) -> PathBuf {
    let dir = repo().join("target/fuzz-lite");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{target}-{n}.{}", case.ext));
    let _ = std::fs::write(&path, &case.bytes);
    path
}

/// Runs cases 0..count of `target`: `make(n)` builds case n, `check` must
/// return without panicking, within `limit`, on a thread with `stack` bytes,
/// and allocate less than BIG_ALLOCATION at once. Panics listing what failed.
pub fn run<M, C>(target: &str, count: usize, stack: usize, limit: Duration, make: M, mut check: C)
where
    M: Fn(usize) -> Case + Send + Sync + 'static,
    C: FnMut(&Case) + Send + 'static,
{
    install_hook();
    let make = Arc::new(make);
    let only: Option<usize> = std::env::var("SONKORE_FUZZ_CASE")
        .ok()
        .and_then(|s| s.strip_prefix(&format!("{target}:")).and_then(|n| n.parse().ok()));
    let range = match only {
        Some(n) => n..n + 1,
        None => 0..count,
    };
    let trace = std::env::var_os("SONKORE_FUZZ_TRACE").is_some();
    let current = Arc::new(AtomicUsize::new(range.start));
    let done = Arc::new(AtomicBool::new(false));
    let failures: Arc<Mutex<Vec<String>>> = Arc::default();
    let slowest = Arc::new(Mutex::new((Duration::ZERO, 0usize)));
    let started = Instant::now();

    let worker = {
        let (make, current, done, failures, slowest) =
            (make.clone(), current.clone(), done.clone(), failures.clone(), slowest.clone());
        let target = target.to_owned();
        let range = range.clone();
        std::thread::Builder::new()
            .name(format!("fuzz-{target}"))
            .stack_size(stack)
            .spawn(move || {
                TARGET.with(|t| t.set(Box::leak(target.clone().into_boxed_str())));
                let mut resolved = 0;
                for n in range {
                    current.store(n, Ordering::Release);
                    let case = make(n);
                    if trace {
                        eprintln!("{target}:{n} {} ({} bytes, .{})", case.label, case.bytes.len(), case.ext);
                    }
                    let t = Instant::now();
                    BIGGEST.with(|b| b.set(0));
                    CASE.with(|c| c.set(n));
                    IN_CASE.with(|c| c.set(true));
                    WATCHING.with(|w| w.set(true));
                    let r = panic::catch_unwind(AssertUnwindSafe(|| check(&case)));
                    WATCHING.with(|w| w.set(false));
                    IN_CASE.with(|c| c.set(false));
                    let took = t.elapsed();
                    {
                        let mut s = slowest.lock().unwrap();
                        if took > s.0 {
                            *s = (took, n);
                        }
                    }
                    let biggest = BIGGEST.with(Cell::get);
                    let why = if r.is_err() {
                        Some(LAST_PANIC.with(|p| std::mem::take(&mut *p.borrow_mut())))
                    } else if biggest > BIG_ALLOCATION {
                        let bt = BIG_WHERE.with(|b| b.borrow_mut().take());
                        let place = match bt {
                            Some(bt) if resolved < 6 => {
                                resolved += 1;
                                frames_of(&bt)
                            }
                            _ => String::new(),
                        };
                        Some(format!("allocated {} MiB at once{place}", biggest >> 20))
                    } else {
                        None
                    };
                    if let Some(why) = why {
                        let saved = save(&target, n, &case);
                        failures.lock().unwrap().push(format!(
                            "{target}:{n} {} ({} bytes) -> {}\n    {}",
                            case.label,
                            case.bytes.len(),
                            saved.display(),
                            why.replace('\n', "\n    ")
                        ));
                    }
                }
                done.store(true, Ordering::Release);
            })
            .unwrap()
    };

    // Watch: a case that stops moving is a loop (or a crawl).
    let mut seen = usize::MAX;
    let mut since = Instant::now();
    while !done.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
        let n = current.load(Ordering::Acquire);
        if n != seen {
            seen = n;
            since = Instant::now();
        } else if since.elapsed() > limit {
            let case = make(n);
            let saved = save(target, n, &case);
            panic!(
                "{target}:{n} {} ({} bytes) did not finish within {limit:?} -> {}",
                case.label,
                case.bytes.len(),
                saved.display()
            );
        }
        if worker.is_finished() && !done.load(Ordering::Acquire) {
            panic!("{target}: the worker thread died at case {n}");
        }
    }
    let _ = worker.join();
    let (slow, slow_n) = *slowest.lock().unwrap();
    eprintln!(
        "{target}: {} cases in {:.2?} (slowest: case {slow_n}, {slow:.2?})",
        count.min(range.len()),
        started.elapsed()
    );
    let failures = failures.lock().unwrap();
    if !failures.is_empty() {
        // One line per distinct place.
        let mut distinct: Vec<&String> = Vec::new();
        let mut places = std::collections::HashSet::new();
        for f in failures.iter() {
            // A panic's location; for an allocation, the code that made it.
            let mut lines = f.lines().skip(1);
            let first = lines.next().unwrap_or("");
            let place = if first.trim_start().starts_with("allocated") { lines.next().unwrap_or(first) } else { first };
            let place = place.to_owned();
            if places.insert(place) {
                distinct.push(f);
            }
        }
        panic!(
            "{target}: {} of {} cases failed ({} distinct):\n{}",
            failures.len(),
            range.len(),
            distinct.len(),
            distinct.iter().take(20).map(|s| s.as_str()).collect::<Vec<_>>().join("\n")
        );
    }
}

/// `run` over a `Plan`.
pub fn run_plan<C>(target: &'static str, plan: Plan, stack: usize, limit: Duration, check: C)
where
    C: FnMut(&Case) + Send + 'static,
{
    if plan.len() == 0 {
        eprintln!("{target}: no seeds, skipped");
        return;
    }
    let count = plan.len();
    run(target, count, stack, limit, move |n| plan.case(target, n), check);
}
