//! The look: the appearance settings — the mini player's own values over
//! the full view's while it is the mini player — resolved into the colours
//! and sizes the
//! Slint `Theme` global shows.

use serde_json::Value as Json;
use slint::{Color, ComponentHandle, Model};
use sonkore_core::settings::Settings;

use crate::{Skin, Skins, Theme};

/// The appearance as resolved for the window now.
#[derive(Clone, Debug)]
pub struct Look {
    pub skin_index: usize,
    pub accent_override: String,
    pub icon_style: String,
    pub density: String,
    pub list_density: String,
    pub alternate_rows: bool,
    pub list_covers: bool,
    pub inverted_skins: Vec<String>,
    pub slider_style: String,
    pub volume_style: String,
    pub backdrop: String,
    pub window_buttons: String,
    pub window_radius: i64,
    pub window_outline: String,
    pub transport_style: String,
    pub visualizer: String,
    /// `Theme.font-family` (fonts.rs; "" is the system's).
    pub font: String,
}

/// A setting: the mini player's own value while
/// compact, else appearance/<key> (ui/visualizer for the visualizer).
fn setting(settings: &Settings, compact_values: &Option<serde_json::Map<String, Json>>, key: &str) -> Option<Json> {
    if let Some(own) = compact_values.as_ref().and_then(|m| m.get(key)) {
        return Some(own.clone());
    }
    let full = if key == "visualizer" { "ui/visualizer".to_owned() } else { format!("appearance/{key}") };
    settings.value(&full).map(|v| Json::String(v.as_text()))
}

fn text_of(v: Option<Json>, fallback: &str) -> String {
    match v {
        Some(Json::String(s)) => s,
        Some(Json::Number(n)) => n.to_string(),
        Some(Json::Bool(b)) => b.to_string(),
        _ => fallback.to_owned(),
    }
}

fn int_of(v: Option<Json>, fallback: i64) -> i64 {
    match v {
        Some(Json::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(fallback, |f| f.round() as i64)),
        Some(Json::String(s)) => s.trim().parse().unwrap_or(fallback),
        _ => fallback,
    }
}

fn bool_of(v: Option<Json>, fallback: bool) -> bool {
    match v {
        Some(Json::Bool(b)) => b,
        Some(Json::String(s)) => {
            let t = s.trim();
            !(t.is_empty() || t == "0" || t.eq_ignore_ascii_case("false"))
        }
        Some(Json::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => fallback,
    }
}

impl Look {
    /// The look from the settings.
    pub fn load(settings: &Settings, skins: usize) -> Look {
        let compact = settings.flag("ui/compactMode", false);
        let compact_values = if compact {
            serde_json::from_str::<Json>(&settings.text("appearanceCompact/values", "")).ok().and_then(|j| match j {
                Json::Object(m) => Some(m),
                _ => None,
            })
        } else {
            None
        };
        let get = |k: &str| setting(settings, &compact_values, k);
        let mut backdrop = text_of(get("backdrop"), "plain");
        if backdrop == "aurora-live" {
            backdrop = "aurora".into();
        }
        Look {
            skin_index: int_of(get("skinIndex"), 0).clamp(0, skins.saturating_sub(1) as i64) as usize,
            accent_override: text_of(get("accentOverride"), ""),
            icon_style: text_of(get("iconStyle"), "solid"),
            density: text_of(get("density"), "comfortable"),
            list_density: text_of(get("listDensity"), ""),
            alternate_rows: bool_of(get("alternateRows"), false),
            list_covers: bool_of(get("listCovers"), true),
            inverted_skins: settings
                .text("appearance/invertedSkins", "")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            slider_style: text_of(get("sliderStyle"), "line"),
            volume_style: text_of(get("volumeStyle"), "line"),
            backdrop,
            window_buttons: text_of(get("windowButtons"), "windows"),
            window_radius: int_of(get("windowRadius"), 8).clamp(0, 24),
            window_outline: text_of(get("windowOutline"), "highlight"),
            transport_style: text_of(get("transportStyle"), "sonkore"),
            visualizer: text_of(get("visualizer"), "bars"),
            font: crate::fonts::choose(&settings.text("appearance/font", "inter")).to_owned(),
        }
    }
}

// --- Colour math -----------------------------------------------------------------------

fn luminance(c: Color) -> f64 {
    0.2126 * c.red() as f64 / 255.0 + 0.7152 * c.green() as f64 / 255.0 + 0.0722 * c.blue() as f64 / 255.0
}

/// Darker (factor·100 %): the HSV value divided by the factor.
fn darker(c: Color, factor: f64) -> Color {
    let (r, g, b) = (c.red() as f64 / 255.0, c.green() as f64 / 255.0, c.blue() as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let s = if max > 0.0 { (max - min) / max } else { 0.0 };
    let h = if max == min {
        0.0
    } else if max == r {
        60.0 * (((g - b) / (max - min)).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / (max - min) + 2.0)
    } else {
        60.0 * ((r - g) / (max - min) + 4.0)
    };
    let v = v * 100.0 / (factor * 100.0).round();
    let c2 = v * s;
    let x = c2 * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c2;
    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c2, x, 0.0),
        1 => (x, c2, 0.0),
        2 => (0.0, c2, x),
        3 => (0.0, x, c2),
        4 => (x, 0.0, c2),
        _ => (c2, 0.0, x),
    };
    let to = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_rgb_u8(to(r1), to(g1), to(b1))
}

/// The frame (sidebar, bars) and pane (behind the lists) of a skin: the
/// pane a shade apart from the frame, enough to see.
pub fn tones(skin: &Skin, inverted: bool) -> (Color, Color) {
    let (bg, side) = (skin.bg, skin.sidebar);
    let apart = |c: Color| luminance(side) - luminance(c) >= 0.0157;
    let mut pane = if luminance(bg) < luminance(side) { bg } else { darker(side, if skin.dark { 1.35 } else { 1.05 }) };
    if !apart(pane) {
        pane = darker(side, 1.6);
    }
    if !apart(pane) {
        pane = if luminance(bg) - luminance(side) >= 0.0157 { bg } else { skin.surface };
    }
    if inverted { (pane, side) } else { (side, pane) }
}

fn parse_color(s: &str) -> Option<Color> {
    let hex = s.trim().strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)),
        8 => Some(Color::from_argb_u8((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)),
        _ => None,
    }
}

/// Shows `look` in the window (the main window, or another window's own
/// `Theme` global).
pub fn apply<C>(window: &C, look: &Look)
where
    C: ComponentHandle,
    for<'a> Skins<'a>: slint::Global<'a, C>,
    for<'a> Theme<'a>: slint::Global<'a, C>,
{
    let skins = window.global::<Skins>().get_list();
    let Some(skin) = skins.row_data(look.skin_index).or_else(|| skins.row_data(0)) else { return };
    let t = window.global::<Theme>();
    let inverted = look.inverted_skins.iter().any(|n| *n == skin.name.as_str());
    let (frame, pane) = tones(&skin, inverted);
    let accent = parse_color(&look.accent_override).unwrap_or(skin.accent);
    let on_accent = if look.accent_override.is_empty() {
        skin.on_accent
    } else if luminance(accent) > 0.55 {
        Color::from_rgb_u8(0, 0, 0)
    } else {
        Color::from_rgb_u8(255, 255, 255)
    };
    t.set_font_family(look.font.as_str().into());
    t.set_dark(skin.dark);
    t.set_bg(skin.bg);
    t.set_frame(frame);
    t.set_pane(pane);
    t.set_surface(skin.surface);
    t.set_surface_hi(skin.surface_hi);
    t.set_hover(skin.hover);
    t.set_border(skin.border);
    t.set_text(skin.text);
    t.set_text_dim(skin.text_dim);
    t.set_text_faint(skin.text_faint);
    t.set_accent(accent);
    t.set_on_accent(on_accent);
    t.set_good(skin.good);
    t.set_warn(skin.warn);
    t.set_radius(skin.radius as f32);
    // Interface density: "auto" is the skin's.
    let compact = if look.density == "auto" { skin.compact } else { look.density == "compact" };
    let spacious = look.density == "spacious";
    t.set_compact(compact);
    t.set_spacious(spacious);
    let list_density = match look.list_density.as_str() {
        d @ ("spacious" | "comfortable" | "compact" | "list") => d.to_owned(),
        _ if compact => "compact".into(),
        _ if spacious => "spacious".into(),
        _ => "comfortable".into(),
    };
    let row_height = match list_density.as_str() {
        "list" => 22.0,
        "compact" => 30.0,
        "spacious" => 68.0,
        _ => 56.0,
    };
    t.set_row_height(row_height);
    t.set_list_density(list_density.into());
    t.set_alternate_rows(look.alternate_rows);
    t.set_list_covers(look.list_covers && row_height >= 40.0);
    // The icon files: the Line and Tile styles use the line glyphs.
    t.set_solid_icons(look.icon_style != "line" && look.icon_style != "tile");
    t.set_icon_style(look.icon_style.clone().into());
    t.set_slider_style(look.slider_style.clone().into());
    t.set_volume_style(look.volume_style.clone().into());
    t.set_transport_style(look.transport_style.clone().into());
    let buttons = ["windows", "mac", "gnome", "breeze", "cinnamon", "minimal"];
    t.set_window_buttons(
        if buttons.contains(&look.window_buttons.as_str()) { look.window_buttons.clone() } else { "windows".into() }
            .into(),
    );
    // Windows rounds the window itself (main.rs, desktop_identity).
    t.set_window_radius(if cfg!(windows) { 0.0 } else { look.window_radius as f32 });
    t.set_window_outline(look.window_outline.clone().into());
    t.set_backdrop(look.backdrop.clone().into());
    t.set_visualizer(look.visualizer.clone().into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darker_matches_reference() {
        // "#0e0e11" darker by 135 % (Sonkore's default skin pane).
        let c = darker(Color::from_rgb_u8(0x0e, 0x0e, 0x11), 1.35);
        assert_eq!((c.red(), c.green(), c.blue()), (10, 10, 13));
    }
}
