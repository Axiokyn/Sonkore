//! The interface font (Settings › General › Font, appearance/font): the
//! families bundled in fonts/ (also imported in main.slint, so Slint draws
//! with them), or the system's. The text widths Rust and the views compute
//! (metrics.rs, TextMetrics) are measured with the chosen one.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

macro_rules! family {
    ($id:literal, $name:literal) => {
        (
            $id,
            $name,
            [
                (400, include_bytes!(concat!("../../../fonts/", $id, "/400.ttf")) as &[u8]),
                (500, include_bytes!(concat!("../../../fonts/", $id, "/500.ttf"))),
                (600, include_bytes!(concat!("../../../fonts/", $id, "/600.ttf"))),
                (700, include_bytes!(concat!("../../../fonts/", $id, "/700.ttf"))),
            ],
        )
    };
}

type Family = (&'static str, &'static str, [(u16, &'static [u8]); 4]);

/// id (the stored value), family name, the four weights.
pub const FONTS: [Family; 8] = [
    family!("inter", "Inter"),
    family!("manrope", "Manrope"),
    family!("plus-jakarta-sans", "Plus Jakarta Sans"),
    family!("figtree", "Figtree"),
    family!("outfit", "Outfit"),
    family!("dm-sans", "DM Sans"),
    family!("nunito-sans", "Nunito Sans"),
    family!("rubik", "Rubik"),
];

/// "The system's font": measured as Inter, which it most often resembles.
const SYSTEM: usize = usize::MAX;

static CHOSEN: AtomicUsize = AtomicUsize::new(0);

/// The stored value as one of FONTS' ids, or "" for the system's.
pub fn id_of(stored: &str) -> &'static str {
    // (Earlier versions called Inter "sonkore-sans".)
    let stored = if stored == "sonkore-sans" { "inter" } else { stored };
    FONTS.iter().find(|(id, _, _)| *id == stored).map_or("", |(id, _, _)| id)
}

/// Uses the font appearance/font names: returns its family name for
/// `Theme.font-family` ("" for the system's).
pub fn choose(stored: &str) -> &'static str {
    let id = id_of(stored);
    let index = FONTS.iter().position(|(i, _, _)| *i == id).unwrap_or(SYSTEM);
    CHOSEN.store(index, Ordering::Relaxed);
    if index == SYSTEM { "" } else { FONTS[index].1 }
}

/// Whether the chosen font is the bundled Inter (or the system's, measured
/// as Inter): the hinted advances table is Inter's.
pub fn is_inter() -> bool {
    matches!(CHOSEN.load(Ordering::Relaxed), 0 | SYSTEM)
}

/// The chosen font's face nearest `weight`.
pub fn face(weight: u16) -> Option<&'static ttf_parser::Face<'static>> {
    static FACES: OnceLock<Vec<Vec<(u16, ttf_parser::Face<'static>)>>> = OnceLock::new();
    let all = FACES.get_or_init(|| {
        FONTS
            .iter()
            .map(|(_, _, weights)| {
                weights.iter().filter_map(|(w, d)| ttf_parser::Face::parse(d, 0).ok().map(|f| (*w, f))).collect()
            })
            .collect()
    });
    let index = CHOSEN.load(Ordering::Relaxed);
    let faces = all.get(if index == SYSTEM { 0 } else { index })?;
    faces.iter().min_by_key(|(w, _)| w.abs_diff(weight)).map(|(_, f)| f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_font_has_its_family_name() {
        for (id, name, weights) in FONTS {
            for (w, data) in weights {
                let face = ttf_parser::Face::parse(data, 0).unwrap_or_else(|e| panic!("{id} {w}: {e}"));
                let family = face
                    .names()
                    .into_iter()
                    .filter(|n| {
                        n.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY || n.name_id == ttf_parser::name_id::FAMILY
                    })
                    .filter_map(|n| n.to_string())
                    .collect::<Vec<_>>();
                assert!(family.iter().any(|f| f == name), "{id} {w}: {family:?}");
            }
        }
    }

    #[test]
    fn stored_values_map_to_families() {
        assert_eq!(id_of("sonkore-sans"), "inter");
        assert_eq!(id_of("manrope"), "manrope");
        assert_eq!(id_of(""), "");
        assert_eq!(id_of("comic"), "");
    }
}
