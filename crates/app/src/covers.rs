//! Covers for the interface: decoded off the
//! interface's thread at the size they are drawn (physical pixels, corners
//! rounded), kept in sonkore-library's byte-bounded cache; the views ask
//! again when one arrives.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode};
use sonkore_library::covers::CoverCache;

struct Covers {
    cache: CoverCache,
    /// The decoded pictures as Slint images (shared, not copied per row).
    images: RefCell<HashMap<(String, u32, u32), Image>>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
    scale: Cell<f32>,
    timer: Timer,
    pending: Cell<bool>,
    scale_timer: Timer,
}

thread_local! {
    static COVERS: Rc<Covers> = {
        // 24 MB of pictures, 2 decoders.
        let cache = CoverCache::new(24 << 20, 2);
        cache.set_ready(|_path| {
            let _ = slint::invoke_from_event_loop(|| COVERS.with(|c| c.arrived()));
        });
        Rc::new(Covers {
            cache,
            images: RefCell::new(HashMap::new()),
            listeners: RefCell::new(Vec::new()),
            scale: Cell::new(1.0),
            timer: Timer::default(),
            pending: Cell::new(false),
            scale_timer: Timer::default(),
        })
    };
}

impl Covers {
    /// Covers come in bursts (a screenful): the views refresh once for them.
    fn arrived(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let me = Rc::downgrade(self);
        self.timer.start(TimerMode::SingleShot, Duration::from_millis(60), move || {
            let Some(c) = me.upgrade() else { return };
            c.pending.set(false);
            let listeners: Vec<Rc<dyn Fn()>> = c.listeners.borrow().clone();
            for l in listeners {
                l();
            }
        });
    }
}

/// The window's scale (covers are decoded at the pixels they cover).
pub fn set_scale(scale: f32) {
    COVERS.with(|c| c.scale.set(scale.max(0.5)));
}

/// Follows the scale `scale` reports (the screen's is known once the window
/// is on it, and changes when it moves to another): the views ask again.
pub fn follow_scale(scale: impl Fn() -> Option<f32> + 'static) {
    COVERS.with(|c| {
        let me = Rc::downgrade(c);
        c.scale_timer.start(TimerMode::Repeated, Duration::from_millis(500), move || {
            let (Some(c), Some(s)) = (me.upgrade(), scale()) else { return };
            let s = s.max(0.5);
            if (c.scale.get() - s).abs() > 1e-3 {
                c.scale.set(s);
                c.images.borrow_mut().clear();
                c.arrived();
            }
        });
    });
}

/// `path`'s cover `side` × `side` logical pixels, corners rounded by
/// `radius`; None while it is decoded (then `on_arrival` listeners run) or
/// when there is none.
pub fn get(path: &str, side: f32, radius: f32) -> Option<Image> {
    if path.is_empty() || side <= 0.0 {
        return None;
    }
    COVERS.with(|c| {
        let s = c.scale.get();
        let (px, r) = ((side * s).round() as u32, (radius * s).round() as u32);
        let key = (path.to_owned(), px, r);
        if let Some(img) = c.images.borrow().get(&key) {
            return Some(img.clone());
        }
        let picture = c.cache.cover(path, px, r)?;
        let buffer =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(picture.as_raw(), picture.width(), picture.height());
        let image = Image::from_rgba8_premultiplied(premultiply(buffer));
        let mut images = c.images.borrow_mut();
        if images.len() > 600 {
            images.clear(); // the cache below keeps the decoded ones
        }
        images.insert(key, image.clone());
        Some(image)
    })
}

fn premultiply(mut buffer: SharedPixelBuffer<Rgba8Pixel>) -> SharedPixelBuffer<Rgba8Pixel> {
    for p in buffer.make_mut_slice() {
        if p.a != 255 {
            let a = p.a as u16;
            p.r = ((p.r as u16 * a + 127) / 255) as u8;
            p.g = ((p.g as u16 * a + 127) / 255) as u8;
            p.b = ((p.b as u16 * a + 127) / 255) as u8;
        }
    }
    buffer
}

/// These tracks' covers changed (one downloaded): decoded again when shown.
pub fn forget(paths: &[String]) {
    COVERS.with(|c| {
        let set: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        c.images.borrow_mut().retain(|(path, _, _), _| !set.contains(path.as_str()));
        c.cache.forget(paths);
        c.arrived();
    });
}

/// Runs `f` (on the interface's thread) when covers asked for have arrived.
pub fn on_arrival(f: impl Fn() + 'static) {
    COVERS.with(|c| c.listeners.borrow_mut().push(Rc::new(f)));
}
