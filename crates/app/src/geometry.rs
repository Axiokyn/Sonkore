//! The main window's size and state in the stored format (widgets/geometry,
//! format 3, the one earlier Sonkore versions wrote), so the window opens as
//! it was left.

/// What the window opens with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

const MAGIC: u32 = 0x1D9D_0CB;

fn rect(b: &[u8]) -> (i32, i32, i32, i32) {
    let v = |i: usize| i32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    (v(0), v(4), v(8), v(12))
}

/// The stored geometry read: the normal (unmaximized) geometry.
pub fn parse(data: &[u8]) -> Option<Geometry> {
    if data.len() < 4 + 2 + 2 + 16 + 16 + 4 + 2 {
        return None;
    }
    if u32::from_be_bytes([data[0], data[1], data[2], data[3]]) != MAGIC {
        return None;
    }
    let major = u16::from_be_bytes([data[4], data[5]]);
    if !(1..=3).contains(&major) {
        return None;
    }
    let (l, t, r, b) = rect(&data[24..40]); // normalGeometry
    let maximized = data[44] != 0;
    let (width, height) = (r - l + 1, b - t + 1);
    (width > 0 && height > 0).then_some(Geometry { x: l, y: t, width: width as u32, height: height as u32, maximized })
}

/// The stored geometry written for a frameless window on screen 0.
pub fn encode(g: &Geometry, screen_width: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(66);
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    let r = |out: &mut Vec<u8>| {
        for v in [g.x, g.y, g.x + g.width as i32 - 1, g.y + g.height as i32 - 1] {
            out.extend_from_slice(&v.to_be_bytes());
        }
    };
    r(&mut out); // frameGeometry (no frame: the same)
    r(&mut out); // normalGeometry
    out.extend_from_slice(&0i32.to_be_bytes()); // screen
    out.push(g.maximized as u8);
    out.push(0); // full screen
    out.extend_from_slice(&screen_width.to_be_bytes());
    r(&mut out); // geometry
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let g = Geometry { x: 0, y: 0, width: 1320, height: 840, maximized: true };
        let data = encode(&g, 2560);
        assert_eq!(data.len(), 66);
        assert_eq!(parse(&data), Some(g));
    }
}
