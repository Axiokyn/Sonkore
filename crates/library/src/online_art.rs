//! Album art for albums that have none on disk, from the
//! Cover Art Archive: the album is looked up on MusicBrainz (only its
//! artist and album names are sent) and the front cover kept in the cache
//! folder (`$XDG_CACHE_HOME/sonkore/covers/<sha1 of the album key>.img`), so
//! it is fetched once. Asked lazily, by the cover loader, for what is shown.
//!
//! One album looked up at a time, one MusicBrainz request a second (its
//! rate limit), three downloads from the archive at a time, waits after a
//! "too many requests" / "busy" answer (Retry-After, or 1, 2, 4, 8, 16 s,
//! five times at most), a "none" marker for albums without art that is
//! tried again 30 days later. Off until the user turns it on
//! (library/onlineCovers), as in earlier versions.
//!
//! Threads: `cached_cover` is called from any thread (the cover loader's);
//! the network work runs on threads of its own. What needs the library (a
//! track's artist and album, an album's tracks) is done in `process`, which
//! the library's owner calls when the notifier fires, as for `Library`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use sonkore_core::paths;
use sonkore_core::settings::Settings;

use crate::library::{Library, write_file};
use crate::meta::TrackMeta;

/// MusicBrainz asks for at most one request a second.
pub const PACE: Duration = Duration::from_millis(1100);
/// An album without art is looked up again after this.
pub const RETRY_NONE: Duration = Duration::from_secs(30 * 24 * 3600);
/// MusicBrainz search score (0–100) worth trusting.
pub const MIN_SCORE: i64 = 70;
/// Downloads from the archive at a time.
pub const PARALLEL_FETCHES: usize = 3;
/// Retries after a "busy" answer.
pub const MAX_ATTEMPTS: u32 = 5;

/// `Sonkore/<version> ( https://github.com/Axiokyn/Sonkore )`: MusicBrainz
/// and the archive want to know who asks and how to reach them.
pub fn user_agent() -> String {
    format!("Sonkore/{} ( https://github.com/Axiokyn/Sonkore )", env!("CARGO_PKG_VERSION"))
}

/// An HTTP answer.
pub struct Response {
    /// 0 when no answer came at all.
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

/// A blocking GET (with the user agent, "Accept: application/json,
/// image/*", a 20 s timeout and safe redirects). Tests give their own.
pub trait Http: Send + Sync {
    fn get(&self, url: &str) -> Response;
}

/// The real one, over ureq.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl Default for UreqHttp {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent(user_agent())
            .http_status_as_error(false)
            .max_redirects(10);
        // Windows: its own TLS (SChannel) and certificate store.
        #[cfg(windows)]
        let config = config.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
        let config = config.build();
        UreqHttp { agent: config.into() }
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str) -> Response {
        match self.agent.get(url).header("Accept", "application/json, image/*").call() {
            Ok(mut r) => {
                let status = r.status().as_u16();
                let retry_after = r.headers().get("Retry-After").and_then(|v| v.to_str().ok()).map(str::to_string);
                let body = r.body_mut().with_config().limit(32 << 20).read_to_vec().unwrap_or_default();
                Response { status, retry_after, body }
            }
            Err(_) => Response { status: 0, retry_after: None, body: Vec::new() },
        }
    }
}

/// How long to wait before asking again after a 429 / 503:
/// None for any other answer, or once `max` retries were made.
pub fn retry_delay(r: &Response, attempt: u32, max: u32) -> Option<Duration> {
    if (r.status != 429 && r.status != 503) || attempt >= max {
        return None;
    }
    let after: i64 = r.retry_after.as_deref().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let seconds = after.max(1 << attempt).min(60);
    Some(Duration::from_secs(seconds as u64))
}

/// sha1(key) as hex, its first 24 characters.
pub fn hash_of(key: &str) -> String {
    let hex = sha1_smol::Sha1::from(key.as_bytes()).digest().to_string();
    hex[..24].to_string()
}

/// Lucene query text: quotes and backslashes would change its meaning.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace(['"', '\\'], ""))
}

/// A track's album key for OnlineArt (`album_key_of`).
fn track_key(t: &TrackMeta) -> Option<String> {
    album_key_of(&t.album_artist, t.display_artist(), &t.album).map(|k| k.0)
}

/// The album key OnlineArt uses (trimmed, album artist else artist), with
/// what is looked up; None without both an artist and an album. Covers are
/// cached and shown by it (not by `TrackMeta::album_key`, which keeps the
/// spaces around a tag).
pub fn album_key_of(album_artist: &str, artist: &str, album: &str) -> Option<(String, String, String)> {
    let album_artist = album_artist.trim();
    let who = if album_artist.is_empty() { artist.trim() } else { album_artist };
    let album = album.trim();
    if who.is_empty() || album.is_empty() {
        return None;
    }
    Some((format!("{}\u{1f}{}", who.to_lowercase(), album.to_lowercase()), who.to_string(), album.to_string()))
}

/// What changed.
#[derive(Clone, Debug, PartialEq)]
pub enum ArtEvent {
    EnabledChanged,
    ProgressChanged,
    /// A cover arrived for these tracks: what shows them reloads.
    CoverArrived(Vec<String>),
}

#[derive(Clone, Debug)]
struct Job {
    key: String,
    artist: String,
    album: String,
}

enum ToOwner {
    /// A track's cover was asked for (by the cover loader).
    Asked(String),
    Searched,
    /// The album is done: its cover file, or none.
    Finished {
        key: String,
        file: Option<String>,
    },
    /// Not answered: asked again another time.
    Failed {
        key: String,
        error: String,
    },
}

struct Shared {
    enabled: AtomicBool,
    by_path: Mutex<HashMap<String, String>>, // track path -> cover file ("" = asked, nothing yet)
    dir: PathBuf,
    generation: Mutex<u64>, // bumped by drop_all: workers drop stale jobs
}

type Notifier = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;

pub struct OnlineArt {
    shared: Arc<Shared>,
    settings: PathBuf,
    to_owner_tx: Sender<ToOwner>,
    to_owner: Receiver<ToOwner>,
    jobs: Sender<(u64, Job)>,
    notifier: Notifier,
    queued: HashSet<String>,
    pending: usize,
    fetched: usize,
    last_error: String,
}

fn notify(n: &Notifier) {
    if let Some(f) = n.lock().unwrap().as_ref() {
        f();
    }
}

impl OnlineArt {
    /// With the standard cache folder and settings file, over the network.
    pub fn standard() -> OnlineArt {
        OnlineArt::new(paths::cache_dir().join("covers"), paths::settings_file(), Arc::new(UreqHttp::default()))
    }

    pub fn new(cache_dir: PathBuf, settings: PathBuf, http: Arc<dyn Http>) -> OnlineArt {
        let enabled = Settings::open(&settings).flag("library/onlineCovers", false);
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(enabled),
            by_path: Mutex::new(HashMap::new()),
            dir: cache_dir,
            generation: Mutex::new(0),
        });
        let (to_owner_tx, to_owner) = mpsc::channel();
        let (jobs, job_rx) = mpsc::channel::<(u64, Job)>();
        let notifier: Notifier = Arc::new(Mutex::new(None));
        {
            let shared = shared.clone();
            let tx = to_owner_tx.clone();
            let notifier = notifier.clone();
            thread::Builder::new()
                .name("sonkore-online-art".into())
                .spawn(move || worker(job_rx, shared, http, tx, notifier))
                .expect("online art thread");
        }
        OnlineArt {
            shared,
            settings,
            to_owner_tx,
            to_owner,
            jobs,
            notifier,
            queued: HashSet::new(),
            pending: 0,
            fetched: 0,
            last_error: String::new(),
        }
    }

    pub fn set_notifier(&mut self, f: impl Fn() + Send + Sync + 'static) {
        *self.notifier.lock().unwrap() = Some(Box::new(f));
    }

    /// For `covers::set_online_lookup`: from any thread, the downloaded
    /// cover of a track's album, or None when there is none yet (the first
    /// ask for an album queues its download, when enabled).
    pub fn lookup(&self) -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        let shared = self.shared.clone();
        let tx = self.to_owner_tx.clone();
        let notifier = self.notifier.clone();
        Arc::new(move |path: &str| {
            {
                let mut map = shared.by_path.lock().unwrap();
                if let Some(file) = map.get(path) {
                    return (!file.is_empty()).then(|| file.clone());
                }
                map.insert(path.to_string(), String::new()); // asked once
            }
            if tx.send(ToOwner::Asked(path.to_string())).is_ok() {
                notify(&notifier);
            }
            None
        })
    }

    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }
    /// Albums queued, being looked up or downloaded.
    pub fn pending(&self) -> usize {
        self.pending
    }
    pub fn fetched(&self) -> usize {
        self.fetched
    }
    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    pub fn cache_dir(&self) -> &PathBuf {
        &self.shared.dir
    }
    pub fn file_for(&self, key: &str) -> PathBuf {
        self.shared.dir.join(format!("{}.img", hash_of(key)))
    }
    fn none_for(&self, key: &str) -> PathBuf {
        self.shared.dir.join(format!("{}.none", hash_of(key)))
    }

    fn drop_all(&mut self) {
        *self.shared.generation.lock().unwrap() += 1;
        self.queued.clear();
        self.pending = 0;
    }

    pub fn set_enabled(&mut self, on: bool) -> Vec<ArtEvent> {
        if on == self.enabled() {
            return Vec::new();
        }
        self.shared.enabled.store(on, Ordering::Relaxed);
        let mut s = Settings::open(&self.settings);
        s.set_flag("library/onlineCovers", on);
        let _ = s.save();
        if !on {
            self.drop_all();
        }
        // Tracks asked for without an answer are asked for again when shown.
        self.shared.by_path.lock().unwrap().retain(|_, f| !f.is_empty());
        vec![ArtEvent::EnabledChanged, ArtEvent::ProgressChanged]
    }

    /// The cache folder's size on disk, in bytes.
    pub fn cache_bytes(&self) -> u64 {
        fs::read_dir(&self.shared.dir)
            .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }

    /// Forgets every downloaded cover and "none" marker.
    pub fn clear_cache(&mut self) -> Vec<ArtEvent> {
        if let Ok(rd) = fs::read_dir(&self.shared.dir) {
            for e in rd.flatten() {
                if e.path().is_file() {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        self.drop_all();
        self.shared.by_path.lock().unwrap().clear();
        self.fetched = 0;
        self.last_error.clear();
        vec![ArtEvent::ProgressChanged]
    }

    /// The library's albums against the cache (one stat per album), so a
    /// cover fetched before shows at once. Call it when the tracks change.
    pub fn reindex(&self, lib: &Library) {
        let mut files: HashMap<String, Option<String>> = HashMap::new();
        let mut by_path = HashMap::new();
        for t in lib.tracks() {
            let Some(key) = track_key(t) else { continue };
            let file = files
                .entry(key.clone())
                .or_insert_with(|| {
                    let f = self.file_for(&key);
                    f.exists().then(|| f.to_string_lossy().into_owned())
                })
                .clone();
            if let Some(f) = file {
                by_path.insert(t.path.clone(), f);
            }
        }
        *self.shared.by_path.lock().unwrap() = by_path;
    }

    fn index_album(&self, lib: &Library, key: &str, file: &str) -> Vec<ArtEvent> {
        let paths: Vec<String> =
            lib.tracks().iter().filter(|t| track_key(t).as_deref() == Some(key)).map(|t| t.path.clone()).collect();
        {
            let mut map = self.shared.by_path.lock().unwrap();
            for p in &paths {
                map.insert(p.clone(), file.to_string());
            }
        }
        if paths.is_empty() { Vec::new() } else { vec![ArtEvent::CoverArrived(paths)] }
    }

    fn request(&mut self, lib: &Library, path: &str) -> Vec<ArtEvent> {
        if !self.enabled() {
            return Vec::new();
        }
        let t = lib.track(path);
        let Some((key, artist, album)) = album_key_of(&t.album_artist, t.display_artist(), &t.album) else {
            return Vec::new(); // nothing safe to look up
        };
        let file = self.file_for(&key);
        if file.exists() {
            // Fetched before (the library changed since the index was made).
            return self.index_album(lib, &key, &file.to_string_lossy());
        }
        let none = self.none_for(&key);
        if let Ok(m) = fs::metadata(&none) {
            let age = m.modified().ok().and_then(|t| SystemTime::now().duration_since(t).ok()).unwrap_or_default();
            if age < RETRY_NONE {
                return Vec::new();
            }
        }
        if !self.queued.insert(key.clone()) {
            return Vec::new();
        }
        self.pending += 1;
        let generation = *self.shared.generation.lock().unwrap();
        let _ = self.jobs.send((generation, Job { key, artist, album }));
        vec![ArtEvent::ProgressChanged]
    }

    /// Applies what the workers and the cover loader sent.
    pub fn process(&mut self, lib: &Library) -> Vec<ArtEvent> {
        let mut ev = Vec::new();
        while let Ok(msg) = self.to_owner.try_recv() {
            match msg {
                ToOwner::Asked(path) => ev.extend(self.request(lib, &path)),
                ToOwner::Searched => ev.push(ArtEvent::ProgressChanged),
                ToOwner::Finished { key, file } => {
                    if !self.queued.remove(&key) {
                        continue; // dropped meanwhile
                    }
                    self.pending = self.pending.saturating_sub(1);
                    if let Some(f) = file {
                        self.fetched += 1;
                        ev.extend(self.index_album(lib, &key, &f));
                    }
                    self.last_error.clear();
                    ev.push(ArtEvent::ProgressChanged);
                }
                ToOwner::Failed { key, error } => {
                    if self.queued.remove(&key) {
                        self.pending = self.pending.saturating_sub(1);
                    }
                    self.last_error = error;
                    ev.push(ArtEvent::ProgressChanged);
                }
            }
        }
        ev
    }
}

fn reason(r: &Response) -> String {
    if r.status > 0 { format!("HTTP {}", r.status) } else { "no connection".into() }
}

/// Percent-encodes a query value.
fn encode(v: &str) -> String {
    percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The lookup worker: MusicBrainz searches one at a time, paced; the
/// downloads on up to PARALLEL_FETCHES threads.
fn worker(
    jobs: Receiver<(u64, Job)>,
    shared: Arc<Shared>,
    http: Arc<dyn Http>,
    tx: Sender<ToOwner>,
    notifier: Notifier,
) {
    let mut last_request: Option<Instant> = None;
    let fetch_slots = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
    let live =
        |generation: u64| *shared.generation.lock().unwrap() == generation && shared.enabled.load(Ordering::Relaxed);
    let send = |m: ToOwner| {
        if tx.send(m).is_ok() {
            notify(&notifier);
        }
    };
    loop {
        let Ok((generation, job)) = jobs.recv() else {
            return;
        };
        if !live(generation) {
            continue;
        }
        let url = format!(
            "https://musicbrainz.org/ws/2/release/?query={}&fmt=json&limit=8",
            encode(&format!("release:{} AND artist:{}", quoted(&job.album), quoted(&job.artist)))
        );
        let mut attempt = 0;
        let answer = loop {
            if let Some(t) = last_request {
                let since = t.elapsed();
                if since < PACE {
                    thread::sleep(PACE - since);
                }
            }
            last_request = Some(Instant::now());
            let r = http.get(&url);
            match retry_delay(&r, attempt, MAX_ATTEMPTS) {
                Some(wait) => {
                    attempt += 1;
                    thread::sleep(wait.saturating_sub(PACE));
                }
                None => break r,
            }
        };
        if !live(generation) {
            continue;
        }
        if answer.status != 200 {
            send(ToOwner::Failed { key: job.key, error: format!("MusicBrainz did not answer: {}", reason(&answer)) });
            continue;
        }
        let root: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
        let (mut official, mut other) = (Vec::new(), Vec::new());
        let mut group = String::new();
        if let Some(Value::Array(releases)) = root.get("releases") {
            for r in releases {
                if r.get("score").and_then(Value::as_i64).unwrap_or(0) < MIN_SCORE {
                    continue;
                }
                if group.is_empty() {
                    group = r.pointer("/release-group/id").and_then(Value::as_str).unwrap_or("").to_string();
                }
                let id = r.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                if r.get("status").and_then(Value::as_str) == Some("Official") {
                    official.push(id)
                } else {
                    other.push(id)
                }
            }
        }
        official.extend(other);
        official.truncate(4);
        let releases = official;
        if group.is_empty() && releases.is_empty() {
            // Not known to MusicBrainz.
            let none = shared.dir.join(format!("{}.none", hash_of(&job.key)));
            let _ = fs::create_dir_all(&shared.dir);
            let _ = fs::write(none, b"");
            send(ToOwner::Finished { key: job.key, file: None });
            continue;
        }
        send(ToOwner::Searched);
        // A download slot, then the archive on a thread of its own.
        {
            let (lock, cv) = &*fetch_slots;
            let mut n = lock.lock().unwrap();
            while *n >= PARALLEL_FETCHES {
                n = cv.wait(n).unwrap();
            }
            *n += 1;
        }
        let shared2 = shared.clone();
        let http2 = http.clone();
        let tx2 = tx.clone();
        let notifier2 = notifier.clone();
        let slots = fetch_slots.clone();
        thread::spawn(move || {
            let msg = fetch(&job, &group, releases, &*http2, &shared2, generation);
            if let Some(m) = msg
                && tx2.send(m).is_ok()
            {
                notify(&notifier2);
            }
            let (lock, cv) = &*slots;
            *lock.lock().unwrap() -= 1;
            cv.notify_one();
        });
    }
}

/// The front cover from the archive: the release group's, then each
/// release's while the answer is "not found".
fn fetch(
    job: &Job,
    group: &str,
    mut releases: Vec<String>,
    http: &dyn Http,
    shared: &Shared,
    generation: u64,
) -> Option<ToOwner> {
    let live = || *shared.generation.lock().unwrap() == generation && shared.enabled.load(Ordering::Relaxed);
    let mut url = if !group.is_empty() {
        format!("https://coverartarchive.org/release-group/{group}/front-500")
    } else {
        format!("https://coverartarchive.org/release/{}/front-500", releases.remove(0))
    };
    let mut attempt = 0;
    loop {
        if !live() {
            return None;
        }
        let r = http.get(&url);
        if let Some(wait) = retry_delay(&r, attempt, MAX_ATTEMPTS) {
            attempt += 1;
            thread::sleep(wait);
            continue;
        }
        if r.status == 200 && r.body.len() > 256 {
            // Whole or not at all: the file being there means "fetched".
            let file = shared.dir.join(format!("{}.img", hash_of(&job.key)));
            if write_file(&file, &r.body).is_ok() {
                let _ = fs::remove_file(shared.dir.join(format!("{}.none", hash_of(&job.key))));
                return Some(ToOwner::Finished {
                    key: job.key.clone(),
                    file: Some(file.to_string_lossy().into_owned()),
                });
            }
            return Some(ToOwner::Failed {
                key: job.key.clone(),
                error: "Cover Art Archive did not answer: no connection".into(),
            });
        }
        let missing = r.status == 404;
        if missing && !releases.is_empty() {
            url = format!("https://coverartarchive.org/release/{}/front-500", releases.remove(0));
            continue;
        }
        if missing {
            // No art for this album anywhere.
            let _ = fs::create_dir_all(&shared.dir);
            let _ = fs::write(shared.dir.join(format!("{}.none", hash_of(&job.key))), b"");
            return Some(ToOwner::Finished { key: job.key.clone(), file: None });
        }
        return Some(ToOwner::Failed {
            key: job.key.clone(),
            error: format!("Cover Art Archive did not answer: {}", reason(&r)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_rules() {
        let r =
            |status, after: Option<&str>| Response { status, retry_after: after.map(str::to_string), body: Vec::new() };
        assert_eq!(retry_delay(&r(429, None), 0, 5), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay(&r(503, None), 3, 5), Some(Duration::from_secs(8)));
        assert_eq!(retry_delay(&r(429, Some("30")), 1, 5), Some(Duration::from_secs(30)));
        assert_eq!(retry_delay(&r(429, Some("300")), 1, 5), Some(Duration::from_secs(60)));
        assert_eq!(retry_delay(&r(429, None), 5, 5), None);
        assert_eq!(retry_delay(&r(404, None), 0, 5), None);
    }

    #[test]
    fn keys_and_names() {
        assert_eq!(
            album_key_of(" ", "Artist ", " Album"),
            Some(("artist\u{1f}album".into(), "Artist".into(), "Album".into()))
        );
        assert_eq!(album_key_of("", "", "Album"), None);
        assert_eq!(hash_of("a\u{1f}b").len(), 24);
        assert_eq!(quoted("a \"b\" \\c"), "\"a b c\"");
        assert!(user_agent().starts_with("Sonkore/"));
    }
}
