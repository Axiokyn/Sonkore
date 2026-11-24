//! Pictures painted on the CPU (anti-aliased shapes at fractional
//! positions), shown in Slint as images at the window's device pixels: the
//! visualizer (SpectrumMeter), the window backdrop and the waveform seek bar.

use slint::{Color, Image, Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect, Shader, SpreadMode, Stroke,
    Transform,
};

// --- Colours ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub fn from_slint(c: Color) -> Rgba {
        Rgba {
            r: c.red() as f32 / 255.0,
            g: c.green() as f32 / 255.0,
            b: c.blue() as f32 / 255.0,
            a: c.alpha() as f32 / 255.0,
        }
    }
    /// The alpha multiplied by `alpha` (alpha · the colour's own).
    pub fn with_alpha(self, alpha: f64) -> Rgba {
        Rgba { a: (alpha.clamp(0.0, 1.0) as f32) * self.a, ..self }
    }
    fn skia(self) -> tiny_skia::Color {
        // 8 bits a channel are enough to paint.
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0;
        tiny_skia::Color::from_rgba(q(self.r), q(self.g), q(self.b), self.a.clamp(0.0, 1.0))
            .unwrap_or(tiny_skia::Color::BLACK)
    }
    fn hsv(self) -> (f64, f64, f64) {
        let (r, g, b) = (self.r as f64, self.g as f64, self.b as f64);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            -1.0
        } else if max == r {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h, if max > 0.0 { d / max } else { 0.0 }, max)
    }
    fn from_hsv(h: f64, s: f64, v: f64, a: f32) -> Rgba {
        if h < 0.0 || s == 0.0 {
            return Rgba { r: v as f32, g: v as f32, b: v as f32, a };
        }
        let c = v * s;
        let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
        let m = v - c;
        let (r, g, b) = match (h / 60.0) as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        Rgba { r: (r + m) as f32, g: (g + m) as f32, b: (b + m) as f32, a }
    }
    /// Lighter by `factor` %: the HSV value times factor/100; past full
    /// value the saturation gives way instead.
    pub fn lighter(self, factor: f64) -> Rgba {
        let (h, mut s, mut v) = self.hsv();
        v = v * factor / 100.0;
        if v > 1.0 {
            s = (s - (v - 1.0)).max(0.0);
            v = 1.0;
        }
        Rgba::from_hsv(h, s, v, self.a)
    }
    /// (hue 0..1 or -1 for greys, saturation, lightness).
    pub fn hsl(self) -> (f64, f64, f64) {
        let (r, g, b) = (self.r as f64, self.g as f64, self.b as f64);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (-1.0, 0.0, l);
        }
        let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - max - min) };
        let (h, _, _) = self.hsv();
        (h / 360.0, s, l)
    }
    pub fn from_hsl(h: f64, s: f64, l: f64) -> Rgba {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let hh = h.rem_euclid(1.0) * 6.0;
        let x = c * (1.0 - (hh.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match hh as i32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        Rgba { r: (r + m) as f32, g: (g + m) as f32, b: (b + m) as f32, a: 1.0 }
    }
}

pub fn solid(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c.skia());
    p.anti_alias = true;
    p
}

/// A vertical gradient from y0 to y1.
pub fn vertical(y0: f32, y1: f32, stops: &[(f32, Rgba)]) -> Paint<'static> {
    let first = stops.first().map_or(tiny_skia::Color::TRANSPARENT, |s| s.1.skia());
    let stops = stops.iter().map(|(t, c)| GradientStop::new(*t, c.skia())).collect();
    let mut p = Paint::default();
    p.anti_alias = true;
    p.shader = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1.max(y0 + 0.001)),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(first));
    p
}

/// A rounded rectangle: the corners as quarter ellipses (Béziers).
pub fn rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let rx = r.min(w / 2.0).max(0.0);
    let ry = r.min(h / 2.0).max(0.0);
    if rx <= 0.0 || ry <= 0.0 {
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            pb.push_rect(rect);
        }
        return;
    }
    const K: f32 = 0.552_284_8;
    let (kx, ky) = (rx * K, ry * K);
    let (r_, b_) = (x + w, y + h);
    pb.move_to(x + rx, y);
    pb.line_to(r_ - rx, y);
    pb.cubic_to(r_ - rx + kx, y, r_, y + ry - ky, r_, y + ry);
    pb.line_to(r_, b_ - ry);
    pb.cubic_to(r_, b_ - ry + ky, r_ - rx + kx, b_, r_ - rx, b_);
    pb.line_to(x + rx, b_);
    pb.cubic_to(x + rx - kx, b_, x, b_ - ry + ky, x, b_ - ry);
    pb.line_to(x, y + ry);
    pb.cubic_to(x, y + ry - ky, x + rx - kx, y, x + rx, y);
    pb.close();
}

/// A painting surface of `w` x `h` logical pixels at `dpr` device pixels each.
pub struct Canvas {
    pub pixmap: Pixmap,
    pub dpr: f32,
}

impl Canvas {
    pub fn new(w: f32, h: f32, dpr: f32) -> Option<Canvas> {
        let pw = (w * dpr).round().max(1.0) as u32;
        let ph = (h * dpr).round().max(1.0) as u32;
        Some(Canvas { pixmap: Pixmap::new(pw, ph)?, dpr })
    }
    fn transform(&self) -> Transform {
        Transform::from_scale(self.dpr, self.dpr)
    }
    pub fn fill(&mut self, pb: PathBuilder, paint: &Paint) {
        if let Some(path) = pb.finish() {
            let t = self.transform();
            self.pixmap.fill_path(&path, paint, FillRule::Winding, t, None);
        }
    }
    pub fn stroke(&mut self, pb: PathBuilder, paint: &Paint, stroke: &Stroke) {
        if let Some(path) = pb.finish() {
            let t = self.transform();
            self.pixmap.stroke_path(&path, paint, stroke, t, None);
        }
    }
    pub fn image(&self) -> Image {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            self.pixmap.data(),
            self.pixmap.width(),
            self.pixmap.height(),
        );
        Image::from_rgba8_premultiplied(buf)
    }
}

// --- The visualizer ------------------------------------------------------------------------

const RISE: f64 = 30.0; // per second: the attack, close to immediate
const FALL: f64 = 6.0; // per second: the release
const PEAK_HOLD: f64 = 0.4; // seconds a peak cap waits before it falls
const GRAVITY: f64 = 2.5; // its fall's acceleration, in heights per second²
const REST: f64 = 0.002; // below this nothing shows
const DOT_ROWS: usize = 6;

/// The spectrum's levels and picture: fast attack, gentle release, peak caps
/// that hold and then fall (bars), mirror, dots and line.
#[derive(Default)]
pub struct SpectrumMeter {
    target: Vec<f64>,
    levels: Vec<f64>,
    peaks: Vec<f64>,
    hold: Vec<f64>,
    speed: Vec<f64>,
    resting: bool,
}

impl SpectrumMeter {
    fn resize(&mut self, n: usize) {
        if n != self.levels.len() {
            self.levels.resize(n, 0.0);
            self.peaks.resize(n, 0.0);
            self.hold.resize(n, 0.0);
            self.speed.resize(n, 0.0);
        }
    }
    pub fn set_target(&mut self, target: &[f32], peaks_on: bool) {
        self.target = target.iter().map(|&v| v as f64).collect();
        self.resize(target.len());
        self.update_resting(peaks_on);
    }
    pub fn advance(&mut self, dt: f64, peaks_on: bool) {
        let dt = dt.clamp(0.0, 0.25); // a stalled frame does not make everything leap
        let (rise, fall) = ((dt * RISE).min(1.0), (dt * FALL).min(1.0));
        for i in 0..self.levels.len() {
            let t = self.target.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let l = &mut self.levels[i];
            *l += (t - *l) * if t > *l { rise } else { fall };
            let l = *l;
            // A cap rests on the highest level a moment, then falls faster and faster.
            if l >= self.peaks[i] {
                self.peaks[i] = l;
                self.hold[i] = PEAK_HOLD;
                self.speed[i] = 0.0;
            } else if self.hold[i] > 0.0 {
                self.hold[i] -= dt;
            } else {
                self.speed[i] += GRAVITY * dt;
                self.peaks[i] = l.max(self.peaks[i] - self.speed[i] * dt);
            }
        }
        self.update_resting(peaks_on);
    }
    fn update_resting(&mut self, peaks_on: bool) {
        self.resting = self.target.iter().all(|&t| t <= REST)
            && self.levels.iter().zip(&self.peaks).all(|(&l, &p)| l <= REST && (!peaks_on || p <= REST));
    }
    pub fn resting(&self) -> bool {
        self.resting
    }

    /// Paints `style` bars, mirror, dots or line, in `color`.
    pub fn paint(&self, c: &mut Canvas, w: f32, h: f32, style: &str, color: Rgba, peaks_on: bool, bar_fraction: f64) {
        let n = self.levels.len();
        if n == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let (w, h, dpr) = (w as f64, h as f64, c.dpr as f64);
        // Bars on the device's pixel grid: the same whole number of pixels
        // wide and starting on a pixel edge; heights stay fractional.
        let snap = |v: f64| (v * dpr).round() / dpr;
        let step = w / n as f64;
        let bar_w = (step * bar_fraction * dpr).round().max(1.0) / dpr;
        let bar_x = |i: usize| snap(i as f64 * step + (step - bar_w) / 2.0);
        let top = color.lighter(115.0);

        if style == "line" {
            // A smooth curve through the bands' tops (Catmull-Rom as
            // Béziers, kept inside), over a fill that fades downwards.
            let pts: Vec<(f64, f64)> =
                (0..n).map(|i| ((i as f64 + 0.5) * step, h - 1.0 - self.levels[i] * (h - 2.0))).collect();
            let curve = |pb: &mut PathBuilder| {
                pb.move_to(pts[0].0 as f32, pts[0].1 as f32);
                for i in 0..n.saturating_sub(1) {
                    let p0 = pts[i.saturating_sub(1)];
                    let p1 = pts[i];
                    let p2 = pts[i + 1];
                    let p3 = pts[(i + 2).min(n - 1)];
                    let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, (p1.1 + (p2.1 - p0.1) / 6.0).clamp(0.5, h - 0.5));
                    let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, (p2.1 - (p3.1 - p1.1) / 6.0).clamp(0.5, h - 0.5));
                    pb.cubic_to(c1.0 as f32, c1.1 as f32, c2.0 as f32, c2.1 as f32, p2.0 as f32, p2.1 as f32);
                }
            };
            let mut area = PathBuilder::new();
            curve(&mut area);
            area.line_to(pts[n - 1].0 as f32, h as f32);
            area.line_to(pts[0].0 as f32, h as f32);
            area.close();
            c.fill(area, &vertical(0.0, h as f32, &[(0.0, color.with_alpha(0.32)), (1.0, color.with_alpha(0.04))]));
            let mut line = PathBuilder::new();
            curve(&mut line);
            let stroke = Stroke {
                width: 1.6,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Stroke::default()
            };
            c.stroke(line, &solid(color), &stroke);
            return;
        }

        if style == "dots" {
            // LED columns: the top lit dot glows in proportion.
            let rows = DOT_ROWS as f64;
            let d = bar_w.min(h / (rows + (rows - 1.0) * 0.25));
            let gap = (h - rows * d) / (rows - 1.0);
            for i in 0..n {
                let x = bar_x(i) + (bar_w - d) / 2.0;
                for r in 0..DOT_ROWS {
                    let lit = (self.levels[i] * rows - r as f64).clamp(0.0, 1.0);
                    let tone = if r == DOT_ROWS - 1 && lit > 0.0 { top } else { color };
                    let mut pb = PathBuilder::new();
                    if let Some(rect) =
                        Rect::from_xywh(x as f32, snap(h - d - r as f64 * (d + gap)) as f32, d as f32, d as f32)
                    {
                        pb.push_oval(rect);
                    }
                    c.fill(pb, &solid(tone.with_alpha(0.14 + 0.86 * lit)));
                }
            }
            return;
        }

        let mirror = style == "mirror";
        let caps = peaks_on && !mirror;
        let cap_h = if caps { 1.5f64.max(bar_w * 0.32) } else { 0.0 };
        let cap_gap = if caps { 1.5 } else { 0.0 };
        let room = h - cap_h - cap_gap; // the bars leave room for their caps
        let mut bars = PathBuilder::new();
        let mut peaks = PathBuilder::new();
        for i in 0..n {
            let x = bar_x(i);
            let bh = (self.levels[i] * room).clamp(2.0f64.min(room), room);
            let y = if mirror { (h - bh) / 2.0 } else { h - bh };
            let r = bar_w.min(bh) / 2.0;
            rounded_rect(&mut bars, x as f32, y as f32, bar_w as f32, bh as f32, r as f32);
            if caps && self.peaks[i] > REST {
                let py = (h - (self.peaks[i] * room).max(bh) - cap_gap - cap_h).max(0.0);
                rounded_rect(&mut peaks, x as f32, py as f32, bar_w as f32, cap_h as f32, (cap_h / 2.0) as f32);
            }
        }
        // Brighter towards the top (from the middle for the mirror).
        let shade = if mirror {
            vertical(0.0, h as f32, &[(0.0, top), (0.5, color.with_alpha(0.55)), (1.0, top)])
        } else {
            vertical(0.0, h as f32, &[(0.0, top), (1.0, color.with_alpha(0.5))])
        };
        c.fill(bars, &shade);
        if caps {
            c.fill(peaks, &solid(top.with_alpha(0.9)));
        }
    }
}

// --- The window backdrop ------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Tone {
    r: f32,
    g: f32,
    b: f32,
    w: f32,
}

fn hash(x: f32, y: f32) -> f32 {
    let (mut a, mut b, mut c) = (x * 0.1031, y * 0.1031, x * 0.1031);
    a -= a.floor();
    b -= b.floor();
    c -= c.floor();
    let d = a * (b + 33.33) + b * (c + 33.33) + c * (a + 33.33);
    a += d;
    b += d;
    c += d;
    let r = (a + b) * c;
    r - r.floor()
}

fn shifted(accent: Rgba, turn: f64) -> Rgba {
    let (h, s, l) = accent.hsl();
    Rgba::from_hsl((h.max(0.0) + turn).rem_euclid(1.0), s.max(0.4), l.max(0.35))
}

/// The decorated window ground (Settings › Appearance › Background: aurora,
/// gradient, glow) for a `w` x `h` logical area at `dpr`: the style's
/// parameters and its picture (smooth field on an 8 px grid,
/// triangular dither of one 8-bit step), opaque, square corners.
pub fn backdrop(style: &str, w: f32, h: f32, dpr: f32, bg: Rgba, accent: Rgba, dark: bool) -> Option<Image> {
    let width = (w * dpr).ceil() as usize;
    let height = (h * dpr).ceil() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let (aurora, glow, gradient) = (style == "aurora", style == "glow", style == "gradient");
    let (wd, hd) = (w as f64, h as f64);
    let span = wd.max(hd);
    let k = if dark { 1.0 } else { 0.7 };
    let (a1, a2, a3) = (accent, shifted(accent, 0.11), shifted(accent, 0.86));
    let tone = |c: Rgba, strength: f64| Tone { r: c.r, g: c.g, b: c.b, w: (strength * k) as f32 };
    let tones = [
        tone(
            a1,
            if aurora {
                0.34
            } else if glow {
                0.32
            } else if gradient {
                0.3
            } else {
                0.0
            },
        ),
        if glow {
            tone(a1, 0.1)
        } else {
            tone(
                a2,
                if aurora {
                    0.28
                } else if gradient {
                    0.1
                } else {
                    0.0
                },
            )
        },
        tone(
            a3,
            if aurora {
                0.2
            } else if gradient {
                0.22
            } else {
                0.0
            },
        ),
    ];
    let blobs: [(f64, f64, f64); 3] = [
        if glow { (wd * 0.5, hd * 1.05, span * 0.6) } else { (wd * 0.12, hd * 0.2, span * 0.7) },
        if glow { (0.0, 0.0, span * 0.4) } else { (wd * 0.804, hd * 0.858, span * 0.65) },
        (wd * 0.559, hd * 0.091, span * 0.45),
    ];
    let s = dpr;
    let (area_x, area_y) = (width as f32 / s, height as f32 / s);
    let area_dot = (area_x * area_x + area_y * area_y).max(1.0);
    const STEP: usize = 8;
    let (gw, gh) = (width / STEP + 2, height / STEP + 2);
    let mut grid = vec![0f32; gw * gh * 3];
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    for gy in 0..gh {
        for gx in 0..gw {
            let px = ((gx * STEP) as f32 + 0.5) / s;
            let py = ((gy * STEP) as f32 + 0.5) / s;
            let (mut r, mut g, mut b) = (bg.r, bg.g, bg.b);
            if gradient {
                let t = ((px * area_x + py * area_y) / area_dot).clamp(0.0, 1.0);
                let lerp = |a: Tone, c: Tone, u: f32| Tone {
                    r: mix(a.r, c.r, u),
                    g: mix(a.g, c.g, u),
                    b: mix(a.b, c.b, u),
                    w: mix(a.w, c.w, u),
                };
                let st = if t < 0.55 {
                    lerp(tones[0], tones[1], t / 0.55)
                } else {
                    lerp(tones[1], tones[2], (t - 0.55) / 0.45)
                };
                r = mix(r, st.r, st.w);
                g = mix(g, st.g, st.w);
                b = mix(b, st.b, st.w);
            } else {
                for (t, blob) in tones.iter().zip(&blobs) {
                    if t.w <= 0.0 {
                        continue;
                    }
                    let (dx, dy) = (px - blob.0 as f32, py - blob.1 as f32);
                    let u = ((dx * dx + dy * dy).sqrt() / (blob.2 as f32).max(1.0)).clamp(0.0, 1.0);
                    let kk = t.w * (1.0 - u);
                    r = mix(r, t.r, kk);
                    g = mix(g, t.g, kk);
                    b = mix(b, t.b, kk);
                }
            }
            let cell = &mut grid[(gy * gw + gx) * 3..][..3];
            cell.copy_from_slice(&[r, g, b]);
        }
    }
    // Triangular dither of about one 8-bit step, from a 256 x 256 tile.
    thread_local! {
        static DITHER: Vec<i32> = (0..256 * 256)
            .map(|i| {
                let (x, y) = ((i % 256) as f32 + 0.5, (i / 256) as f32 + 0.5);
                let n = hash(x, y) + hash(x + 17.17, y + 17.17) - 1.0;
                (n * 65536.0).round() as i32
            })
            .collect();
    }
    const FIXED: f32 = 255.0 * 65536.0;
    let level = |v: i32| ((v + 32768) >> 16).clamp(0, 255) as u8;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(width as u32, height as u32);
    let pixels = buf.make_mut_slice();
    let mut row = vec![0i32; gw * 3];
    DITHER.with(|dither| {
        for y in 0..height {
            let gy = y / STEP;
            let fy = (y % STEP) as f32 / STEP as f32;
            let a = &grid[gy * gw * 3..][..gw * 3];
            let b = &grid[(gy + 1) * gw * 3..][..gw * 3];
            for i in 0..gw * 3 {
                row[i] = ((a[i] + (b[i] - a[i]) * fy) * FIXED) as i32;
            }
            let noise = &dither[(y & 255) * 256..][..256];
            let line = &mut pixels[y * width..][..width];
            let mut x0 = 0;
            while x0 < width {
                let c0 = &row[(x0 / STEP) * 3..][..6];
                let (dr, dg, db) =
                    ((c0[3] - c0[0]) / STEP as i32, (c0[4] - c0[1]) / STEP as i32, (c0[5] - c0[2]) / STEP as i32);
                let (mut r, mut g, mut bl) = (c0[0], c0[1], c0[2]);
                for x in x0..(x0 + STEP).min(width) {
                    let n = noise[x & 255];
                    line[x] = Rgba8Pixel { r: level(r + n), g: level(g + n), b: level(bl + n), a: 255 };
                    r += dr;
                    g += dg;
                    bl += db;
                }
                x0 += STEP;
            }
        }
    });
    Some(Image::from_rgba8(buf))
}

/// Paints the main window's ground again when what it depends on changed
/// (the picture is kept between calls: a resize to the same size, or a
/// colour that came back, costs nothing).
pub fn refresh_backdrop(window: &crate::MainWindow) {
    use slint::ComponentHandle;
    thread_local! {
        static LAST: std::cell::RefCell<Option<(String, u32, u32, u32, [u8; 8], bool)>> = const { std::cell::RefCell::new(None) };
    }
    let theme = window.global::<crate::Theme>();
    let style = theme.get_backdrop().to_string();
    if !theme.get_decorated() {
        LAST.with(|l| *l.borrow_mut() = None);
        theme.set_backdrop_image(Image::default());
        return;
    }
    let dpr = window.window().scale_factor();
    let size = window.window().size();
    let (accent, bg, dark) = (theme.get_accent(), theme.get_bg(), theme.get_dark());
    let colours =
        [accent.red(), accent.green(), accent.blue(), accent.alpha(), bg.red(), bg.green(), bg.blue(), bg.alpha()];
    let key = (style.clone(), size.width, size.height, (dpr * 1000.0) as u32, colours, dark);
    if LAST.with(|l| l.borrow().as_ref() == Some(&key)) {
        return;
    }
    let (w, h) = (size.width as f32 / dpr, size.height as f32 / dpr);
    if let Some(image) = backdrop(&style, w, h, dpr, Rgba::from_slint(bg), Rgba::from_slint(accent), dark) {
        theme.set_backdrop_image(image);
        LAST.with(|l| *l.borrow_mut() = Some(key));
    }
}

// --- The waveform seek bar ----------------------------------------------------------------

/// The made-up wave of the "wave" style before a track has one.
pub fn resting_wave() -> Vec<f32> {
    let n = 300;
    let mut seed: i64 = 7;
    (0..n)
        .map(|i| {
            seed = (seed * 1103515245 + 12345) % 2147483648;
            let swell = 0.6 + 0.25 * (i as f64 / n as f64 * std::f64::consts::PI * 3.2).sin();
            let edge = 1f64.min(i as f64 / (n as f64 * 0.04)).min((n - 1 - i) as f64 / (n as f64 * 0.04));
            let v = swell * (0.55 + 0.45 * seed as f64 / 2147483648.0) * edge;
            (v * v) as f32
        })
        .collect()
}

/// The waveform's 1 px lines every 2 px, both ways from the middle, in one
/// colour; the seek bar shows one picture in the accent over another in the
/// rest colour, cut at the position.
pub fn wave(peaks: &[f32], w: f32, h: f32, dpr: f32, color: Rgba) -> Option<Image> {
    let mut c = Canvas::new(w, h, dpr)?;
    let n = peaks.len();
    let mid = h / 2.0;
    let mut pb = PathBuilder::new();
    let mut x = 0.0f32;
    while x + 1.0 <= w {
        let mut v = 0.0f32;
        if n > 0 {
            let a = (x / w * n as f32) as usize;
            let b = (a + 1).max(((x + 2.0) / w * n as f32) as usize);
            for p in peaks.iter().take(b.min(n)).skip(a) {
                v = v.max(*p);
            }
        }
        let half = (v * mid).max(0.5);
        if let Some(r) = Rect::from_xywh(x, mid - half, 1.0, half * 2.0) {
            pb.push_rect(r);
        }
        x += 2.0;
    }
    c.fill(pb, &solid(color));
    Some(c.image())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighter_matches_reference() {
        // (0x1d, 0xb9, 0x54) lighter by 115 %: the value 185 -> 213, the hue and saturation kept.
        let c = Rgba { r: 0x1d as f32 / 255.0, g: 0xb9 as f32 / 255.0, b: 0x54 as f32 / 255.0, a: 1.0 }.lighter(115.0);
        let q = |v: f32| (v * 255.0).round() as i32;
        assert_eq!(q(c.g), 213);
        assert!((q(c.r) - 33).abs() <= 1 && (q(c.b) - 97).abs() <= 1);
    }

    #[test]
    fn meter_rests_after_falling() {
        let mut m = SpectrumMeter::default();
        m.set_target(&[0.8; 4], true);
        m.advance(0.1, true);
        assert!(!m.resting());
        m.set_target(&[0.0; 4], true);
        for _ in 0..200 {
            m.advance(0.033, true);
        }
        assert!(m.resting());
    }
}

/// Paints the waveform seek bars again (the player bar's and the mini
/// player's) from the track's peaks; the made-up wave with nothing loaded.
pub fn refresh_wave(window: &crate::MainWindow) {
    use slint::ComponentHandle;
    let theme = window.global::<crate::Theme>();
    let on = theme.get_slider_style() == "wave";
    crate::waveform::set_active(on);
    if !on {
        return;
    }
    let peaks = if window.global::<crate::App>().get_loaded() { crate::waveform::peaks() } else { resting_wave() };
    let dpr = window.window().scale_factor();
    let played = Rgba::from_slint(theme.get_accent());
    let rest = Rgba::from_slint(theme.get_text()).with_alpha(if theme.get_dark() { 0.22 } else { 0.28 });
    let visual = window.global::<crate::Visual>();
    let bar = visual.get_seek_width();
    if bar > 0.0 {
        visual.set_seek_rest(wave(&peaks, bar, 20.0, dpr, rest).unwrap_or_default());
        visual.set_seek_played(wave(&peaks, bar, 20.0, dpr, played).unwrap_or_default());
    }
    let mini = visual.get_mini_seek_width();
    if mini > 0.0 {
        visual.set_mini_seek_rest(wave(&peaks, mini, 20.0, dpr, rest).unwrap_or_default());
        visual.set_mini_seek_played(wave(&peaks, mini, 20.0, dpr, played).unwrap_or_default());
    }
}
