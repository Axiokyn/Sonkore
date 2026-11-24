//! Covers: a track's picture, square, `side` pixels, its corners rounded by
//! `radius`: embedded artwork first (FLAC PICTURE / ID3 APIC / MP4 covr…),
//! then cover.jpg & co. in its folder, then a cover fetched from the
//! internet (OnlineArt, asked for when there is nothing on disk).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Cursor;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};

use crate::metadata;

/// Where online covers come from, for `load_cover`: the track's cached
/// cover file, or None (asking for it when it is new). Set by the owner of
/// an `OnlineArt`.
type OnlineLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

static ONLINE: OnceLock<Mutex<Option<OnlineLookup>>> = OnceLock::new();

/// Lets `load_cover` use (and ask for) covers from the internet.
pub fn set_online_lookup(f: Option<OnlineLookup>) {
    *ONLINE.get_or_init(|| Mutex::new(None)).lock().unwrap() = f;
}

fn online_cover(path: &str) -> Option<String> {
    let f = ONLINE.get()?.lock().unwrap().clone()?;
    f(path)
}

/// Decodes an image with its EXIF orientation applied (setAutoTransform).
fn decode(bytes: Vec<u8>) -> Option<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Some(img)
}

fn load_source(path: &str) -> Option<DynamicImage> {
    let embedded = metadata::embedded_cover(path);
    if !embedded.is_empty()
        && let Some(img) = decode(embedded)
    {
        return Some(img);
    }
    let dir = match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    };
    let file = metadata::folder_cover(&dir).or_else(|| online_cover(path))?;
    decode(std::fs::read(file).ok()?)
}

/// The centre square of `source` at side × side, corners rounded. Cut
/// first, then scaled: a long thin picture is never scaled whole.
fn square_cover(source: &DynamicImage, side: u32, radius: u32) -> RgbaImage {
    let (w, h) = (source.width(), source.height());
    let cut = w.min(h).max(1);
    let centre = source.crop_imm((w - cut.min(w)) / 2, (h - cut.min(h)) / 2, cut, cut);
    let mut square = centre.resize_exact(side, side, image::imageops::FilterType::Triangle).to_rgba8();
    if radius > 0 {
        round_corners(&mut square, radius as f32);
    }
    square
}

/// Antialiased rounded corners: each corner pixel keeps the part of it
/// inside the circle (4 × 4 samples).
fn round_corners(img: &mut RgbaImage, r: f32) {
    let (w, h) = (img.width(), img.height());
    let span = r.ceil() as u32;
    for y in 0..h {
        for x in 0..w {
            let in_x = x < span || x + span >= w;
            let in_y = y < span || y + span >= h;
            if !(in_x && in_y) {
                continue;
            }
            let (cx, cy) = (if x < span { r } else { w as f32 - r }, if y < span { r } else { h as f32 - r });
            let mut inside = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let dx = if (x < span && px < cx) || (x >= span && px > cx) { px - cx } else { 0.0 };
                    let dy = if (y < span && py < cy) || (y >= span && py > cy) { py - cy } else { 0.0 };
                    if dx * dx + dy * dy <= r * r {
                        inside += 1;
                    }
                }
            }
            let p = img.get_pixel_mut(x, y);
            p[3] = ((p[3] as u32 * inside + 8) / 16) as u8;
        }
    }
}

/// A track's cover, square, `side` pixels (8–2048), corners rounded by
/// `radius`; None when there is none. Safe from any thread.
pub fn load_cover(path: &str, side: u32, radius: u32) -> Option<RgbaImage> {
    let side = side.clamp(8, 2048);
    let source = load_source(path)?;
    Some(square_cover(&source, side, radius.min(side / 2)))
}

/// A cache of decoded covers bounded in bytes, by `path@<side>r<radius>`.
struct Lru {
    map: HashMap<String, Arc<RgbaImage>>,
    order: VecDeque<String>,
    bytes: usize,
    max: usize,
}

impl Lru {
    fn get(&mut self, k: &str) -> Option<Arc<RgbaImage>> {
        let hit = self.map.get(k)?.clone();
        if let Some(i) = self.order.iter().position(|o| o == k) {
            let key = self.order.remove(i).unwrap();
            self.order.push_back(key);
        }
        Some(hit)
    }
    fn insert(&mut self, k: String, v: Arc<RgbaImage>) {
        let cost = (v.as_raw().len()).max(1);
        if cost > self.max {
            return;
        }
        if let Some(old) = self.map.insert(k.clone(), v) {
            self.bytes -= old.as_raw().len().max(1);
            self.order.retain(|o| o != &k);
        }
        self.bytes += cost;
        self.order.push_back(k);
        while self.bytes > self.max {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(v) = self.map.remove(&oldest) {
                self.bytes -= v.as_raw().len().max(1);
            }
        }
    }
    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self.map.keys().filter(|k| !keep(k)).cloned().collect();
        for k in gone {
            if let Some(v) = self.map.remove(&k) {
                self.bytes -= v.as_raw().len().max(1);
            }
            self.order.retain(|o| o != &k);
        }
    }
}

fn key(path: &str, side: u32, radius: u32) -> String {
    format!("{path}@{side}r{radius}")
}

fn key_path(k: &str) -> &str {
    &k[..k.rfind('@').unwrap_or(k.len())]
}

struct CacheState {
    lru: Lru,
    none: HashSet<String>,
    pending: HashSet<String>,
}

type Ready = Arc<dyn Fn(&str) + Send + Sync>;

/// Covers decoded off the interface's thread at the size they are shown,
/// kept in a cache bounded in bytes: `cover`
/// answers at once, with the picture when it is ready, and the `ready`
/// callback (any thread) follows when it is.
pub struct CoverCache {
    state: Arc<Mutex<CacheState>>,
    jobs: Sender<(String, String, u32, u32)>,
    ready: Arc<Mutex<Option<Ready>>>,
}

impl CoverCache {
    /// `max_bytes`: 12 MB in the app; `threads` decoding at once.
    pub fn new(max_bytes: usize, threads: usize) -> CoverCache {
        let state = Arc::new(Mutex::new(CacheState {
            lru: Lru { map: HashMap::new(), order: VecDeque::new(), bytes: 0, max: max_bytes },
            none: HashSet::new(),
            pending: HashSet::new(),
        }));
        let ready: Arc<Mutex<Option<Ready>>> = Arc::new(Mutex::new(None));
        let (jobs, rx) = mpsc::channel::<(String, String, u32, u32)>();
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..threads.max(1) {
            let rx = rx.clone();
            let state = state.clone();
            let ready = ready.clone();
            thread::Builder::new()
                .name("sonkore-covers".into())
                .spawn(move || {
                    loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok((k, path, side, radius)) = job else {
                            return;
                        };
                        let image = load_cover(&path, side, radius);
                        {
                            let mut st = state.lock().unwrap();
                            st.pending.remove(&k);
                            match image {
                                Some(img) => st.lru.insert(k, Arc::new(img)),
                                None => {
                                    st.none.insert(k);
                                    continue;
                                }
                            }
                        }
                        let cb = ready.lock().unwrap().clone();
                        if let Some(cb) = cb {
                            cb(&path);
                        }
                    }
                })
                .expect("cover thread");
        }
        CoverCache { state, jobs, ready }
    }

    /// Called (from a decoding thread) when a track's cover is ready.
    pub fn set_ready(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.ready.lock().unwrap() = Some(Arc::new(f));
    }

    /// The cover now if it is decoded; else None, and it is decoded (once)
    /// unless known to be missing.
    pub fn cover(&self, path: &str, side: u32, radius: u32) -> Option<Arc<RgbaImage>> {
        if path.is_empty() || side == 0 {
            return None;
        }
        let k = key(path, side, radius);
        let mut st = self.state.lock().unwrap();
        if let Some(hit) = st.lru.get(&k) {
            return Some(hit);
        }
        if st.none.contains(&k) || st.pending.contains(&k) {
            return None;
        }
        st.pending.insert(k.clone());
        drop(st);
        let _ = self.jobs.send((k, path.to_string(), side, radius));
        None
    }

    /// The cover decoded here and now:
    /// `id` is `r<radius>/<percent-encoded path>` (or a bare encoded path),
    /// "?v=N" after it ignored; `requested` the larger side wanted (512
    /// without one). A missing cover is a 1 × 1 transparent image.
    pub fn request(&self, id: &str, requested: Option<u32>) -> Arc<RgbaImage> {
        let mut radius = 0u32;
        let mut encoded = id;
        if let Some(rest) = id.strip_prefix('r')
            && let Some(slash) = rest.find('/')
            && let Ok(r) = rest[..slash].parse::<u32>()
            && slash > 0
        {
            radius = r;
            encoded = &rest[slash + 1..];
        }
        if let Some(q) = encoded.rfind('?') {
            encoded = &encoded[..q];
        }
        let path = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy().into_owned();
        let side = requested.unwrap_or(512).clamp(8, 2048);
        let radius = radius.min(side / 2);
        let k = key(&path, side, radius);
        if let Some(hit) = self.state.lock().unwrap().lru.get(&k) {
            return hit;
        }
        let image = Arc::new(load_cover(&path, side, radius).unwrap_or_else(|| RgbaImage::new(1, 1)));
        self.state.lock().unwrap().lru.insert(k, image.clone());
        image
    }

    /// A cover arrived for these tracks: what was kept for them goes, and
    /// `ready` is called for each.
    pub fn forget(&self, paths: &[String]) {
        let set: HashSet<&str> = paths.iter().map(String::as_str).collect();
        {
            let mut st = self.state.lock().unwrap();
            st.lru.retain(|k| !set.contains(key_path(k)));
            st.none.retain(|k| !set.contains(key_path(k)));
        }
        let cb = self.ready.lock().unwrap().clone();
        if let Some(cb) = cb {
            for p in paths {
                cb(p);
            }
        }
    }
}

/// Whether a cover file can be decoded (for tools).
pub fn decodes(path: &Path) -> bool {
    std::fs::read(path).ok().and_then(decode).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_square_cut_before_scaling() {
        // 30 × 10: blue sides, the red centre square. Scaled whole first,
        // the blue ran into the square's edges (and a 3000 × 10 picture
        // became 153600 × 512 on the way).
        let mut img = RgbaImage::from_pixel(30, 10, image::Rgba([0, 0, 255, 255]));
        for y in 0..10 {
            for x in 10..20 {
                img.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }
        let sq = square_cover(&DynamicImage::ImageRgba8(img), 8, 0);
        assert_eq!(sq.dimensions(), (8, 8));
        for (x, y, p) in sq.enumerate_pixels() {
            assert_eq!(p.0, [255, 0, 0, 255], "({x}, {y})");
        }
        let thin = DynamicImage::ImageRgba8(RgbaImage::from_pixel(3000, 10, image::Rgba([9, 9, 9, 255])));
        assert_eq!(square_cover(&thin, 512, 16).dimensions(), (512, 512));
    }

    #[test]
    fn rounded_square() {
        let src = DynamicImage::ImageRgba8(RgbaImage::from_pixel(60, 40, image::Rgba([255, 0, 0, 255])));
        let sq = square_cover(&src, 32, 8);
        assert_eq!(sq.dimensions(), (32, 32));
        assert_eq!(sq.get_pixel(0, 0)[3], 0);
        assert_eq!(sq.get_pixel(16, 16)[3], 255);
        assert_eq!(sq.get_pixel(16, 0)[3], 255);
    }
}
