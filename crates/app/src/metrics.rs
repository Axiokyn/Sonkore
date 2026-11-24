//! Text widths in the interface font (fonts.rs) (horizontal advances, with
//! absolute letter spacing): for the layouts computed from text (column
//! widths, badges).

/// The advance of `text` at `px` pixels and `weight`, `spacing` px after
/// each character.
pub fn width(text: &str, px: f32, weight: u16, spacing: f32) -> f32 {
    let Some(face) = crate::fonts::face(weight) else { return text.chars().count() as f32 * px * 0.55 };
    let scale = px / face.units_per_em() as f32;
    text.chars()
        .map(|c| {
            face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).map_or(px * 0.55, |a| a as f32 * scale)
                + spacing
        })
        .sum()
}

/// A badge's width: 10 px semi-bold, 0.4 spacing, + 12.
pub fn badge_width(text: &str) -> f32 {
    width(text, 10.0, 600, 0.4) + 12.0
}
