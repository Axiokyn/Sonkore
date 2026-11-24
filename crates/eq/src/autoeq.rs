//! AutoEQ (github.com/jaakkopasanen/AutoEq) headphone correction profiles:
//! the index of results, its local cache, search, and the downloads.
//!
//! The downloads block; the UI runs them on a background thread and hands
//! the text to the [`EqController`](crate::EqController). Nothing here is
//! needed offline except the cache.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;

/// Where the results live (raw files of the repository's master branch).
pub const RAW_BASE: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/";
/// The index's cache in the data folder.
pub const CACHE_FILE: &str = "autoeq-index.md";
/// The cache is used while it is younger than this many (calendar) days.
pub const CACHE_DAYS: i64 = 14;
/// Search results shown at most.
pub const MAX_RESULTS: usize = 150;

/// One headphone (or IEM) in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// "Sennheiser HD 650".
    pub name: String,
    /// The results folder, percent-encoded as in the index
    /// ("oratory1990/over-ear/Sennheiser%20HD%20650").
    pub path: String,
    /// Who measured it ("oratory1990").
    pub source: String,
    /// The measurement rig, when given ("GRAS 43AG-7").
    pub rig: String,
    search_text: String,
}

impl Entry {
    /// The profile's URL: `<RAW_BASE><path>/<leaf>%20ParametricEQ.txt`.
    pub fn profile_url(&self) -> String {
        let leaf = self.path.rsplit('/').next().unwrap_or(""); // already percent-encoded
        format!("{RAW_BASE}{}/{leaf}%20ParametricEQ.txt", self.path)
    }
}

/// What a search shows for an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub name: String,
    pub source: String,
    pub rig: String,
}

// "- [<Model>](./<source>/<type>/<Model, URL-encoded>) by <source> on <rig>"
static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^- \[(.+)\]\(\./(.+)\) by ([^\t\n\x0B\f\r ]+)(?: on (.+))?$").unwrap());

/// The index (INDEX.md), parsed.
#[derive(Clone, Debug, Default)]
pub struct Index {
    entries: Vec<Entry>,
}

impl Index {
    pub fn parse(markdown: &str) -> Index {
        let entries = markdown
            .split('\n')
            .filter_map(|raw| {
                let m = LINE.captures(raw.trim())?;
                let group = |i: usize| m.get(i).map_or(String::new(), |g| g.as_str().to_owned());
                let (name, path, source, rig) = (group(1), group(2), group(3), group(4));
                let search_text = format!("{name} {source} {rig}").to_lowercase();
                Some(Entry { name, path, source, rig, search_text })
            })
            .collect();
        Index { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entries whose name, source and rig contain every word of `text`
    /// (case-insensitive), in index order, at most [`MAX_RESULTS`]. No
    /// words, no results.
    pub fn search(&self, text: &str) -> Vec<usize> {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower.split(' ').filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            return Vec::new();
        }
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| words.iter().all(|w| e.search_text.contains(w)))
            .map(|(i, _)| i)
            .take(MAX_RESULTS)
            .collect()
    }
}

/// The index's cache file in `data_dir`.
pub fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE)
}

/// The cached index when it was written fewer than [`CACHE_DAYS`] calendar
/// days ago (local time, counting calendar days between the dates).
pub fn read_fresh_cache(data_dir: &Path) -> Option<String> {
    let path = cache_path(data_dir);
    let modified = fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    if days_between(modified, SystemTime::now()) >= CACHE_DAYS {
        return None;
    }
    fs::read(&path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Stores a freshly downloaded index.
pub fn write_cache(data_dir: &Path, markdown: &str) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    fs::write(cache_path(data_dir), markdown)
}

/// Midnights between two instants, in local time.
fn days_between(from: SystemTime, to: SystemTime) -> i64 {
    local_day(to) - local_day(from)
}

/// Days since 1970-01-01 of the local date of `t`.
fn local_day(t: SystemTime) -> i64 {
    let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    match local_date(secs) {
        Some((y, m, d)) => days_from_civil(y, m, d),
        None => secs.div_euclid(86400),
    }
}

/// The local (year, month, day) of a Unix time.
#[cfg(unix)]
fn local_date(secs: i64) -> Option<(i64, i64, i64)> {
    let time = secs as libc::time_t;
    // SAFETY: localtime_r only writes the tm we hand it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return None;
    }
    Some((i64::from(tm.tm_year) + 1900, i64::from(tm.tm_mon) + 1, i64::from(tm.tm_mday)))
}

/// The local (year, month, day) of a Unix time (the UCRT's _localtime64_s).
#[cfg(windows)]
fn local_date(secs: i64) -> Option<(i64, i64, i64)> {
    #[repr(C)]
    #[derive(Default)]
    struct Tm {
        sec: i32,
        min: i32,
        hour: i32,
        mday: i32,
        mon: i32,
        year: i32,
        wday: i32,
        yday: i32,
        isdst: i32,
    }
    unsafe extern "C" {
        fn _localtime64_s(tm: *mut Tm, time: *const i64) -> i32;
    }
    let mut tm = Tm::default();
    // SAFETY: _localtime64_s only writes the tm we hand it.
    if unsafe { _localtime64_s(&mut tm, &secs) } != 0 {
        return None;
    }
    Some((i64::from(tm.year) + 1900, i64::from(tm.mon) + 1, i64::from(tm.mday)))
}

/// Howard Hinnant's days_from_civil.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

// --- Downloads ----------------------------------------------------------------------

/// A failed download, as text for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadError(pub String);

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DownloadError {}

/// A tolerant URL: bytes that cannot appear in a URL are
/// percent-encoded; existing escapes are kept.
fn tolerant_url(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for &b in url.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// GET `url` as text (blocking).
pub fn fetch(url: &str) -> Result<String, DownloadError> {
    let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60)));
    // Windows: its own TLS (SChannel) and certificate store.
    #[cfg(windows)]
    let config = config.tls_config(
        ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build(),
    );
    let agent = config.build().new_agent();
    let mut response = agent.get(&tolerant_url(url)).call().map_err(|e| DownloadError(e.to_string()))?;
    response.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().map_err(|e| DownloadError(e.to_string()))
}

/// Downloads INDEX.md (blocking).
pub fn download_index() -> Result<String, DownloadError> {
    fetch(&format!("{RAW_BASE}INDEX.md"))
}

/// Downloads an entry's ParametricEQ.txt (blocking).
pub fn download_profile(entry: &Entry) -> Result<String, DownloadError> {
    fetch(&entry.profile_url())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "# Index\n\n\
        - [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650) by oratory1990\n\
        - [Sennheiser HD 600](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600) by crinacle on GRAS 43AG-7\n  \
        - [Moondrop Blessing 2](./crinacle/711%20in-ear/Moondrop%20Blessing%202) by crinacle on 711\r\n\
        * not an entry\n";

    #[test]
    fn parses_and_searches() {
        let index = Index::parse(INDEX);
        assert_eq!(index.len(), 3);
        let e = &index.entries()[1];
        assert_eq!(e.name, "Sennheiser HD 600");
        assert_eq!(e.path, "crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600");
        assert_eq!(e.source, "crinacle");
        assert_eq!(e.rig, "GRAS 43AG-7");
        assert_eq!(
            e.profile_url(),
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600/Sennheiser%20HD%20600%20ParametricEQ.txt"
        );
        assert_eq!(index.search("  SENNHEISER  "), vec![0, 1]);
        assert_eq!(index.search("hd crinacle"), vec![1]);
        assert_eq!(index.search("gras"), vec![1]);
        assert!(index.search("   ").is_empty());
        assert!(index.search("hd 800").is_empty());
    }

    #[test]
    fn search_is_capped() {
        let md: String = (0..400).map(|i| format!("- [Model {i}](./src/over-ear/Model%20{i}) by src\n")).collect();
        assert_eq!(Index::parse(&md).search("model").len(), MAX_RESULTS);
    }

    #[test]
    fn cache_freshness() {
        let dir = std::env::temp_dir().join(format!("sonkore-eq-autoeq-{}", std::process::id()));
        write_cache(&dir, INDEX).unwrap();
        assert_eq!(read_fresh_cache(&dir).as_deref(), Some(INDEX));
        let old = SystemTime::now() - Duration::from_secs(15 * 86400);
        fs::File::options().write(true).open(cache_path(&dir)).unwrap().set_modified(old).unwrap();
        assert!(read_fresh_cache(&dir).is_none());
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
    }

    #[test]
    fn urls_are_made_valid() {
        assert_eq!(tolerant_url("https://x/a%20b/c d/é"), "https://x/a%20b/c%20d/%C3%A9");
    }
}
