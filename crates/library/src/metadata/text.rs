//! Text helpers of the tag readers: ID3 text decoding and the regular
//! expressions they read numbers with (ASCII \d and \s, without Unicode
//! properties).

/// UTF-16 code units to text; unpaired surrogates become U+FFFD.
pub(crate) fn utf16(units: impl Iterator<Item = u16>) -> String {
    char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect()
}

/// Latin-1 bytes to text.
pub(crate) fn latin1(data: &[u8]) -> String {
    data.iter().map(|&b| b as char).collect()
}

/// An ID3 text field: 0 Latin-1, 1 UTF-16 with BOM (host order, little
/// endian here, without one), 2 UTF-16BE, else UTF-8. Cut at the first NUL
/// (a v2.4 multi-value field keeps its first value), trimmed.
pub(crate) fn decode_id3_text(data: &[u8], encoding: u8) -> String {
    let s = match encoding {
        0 => latin1(data),
        1 => {
            let (big, body) = if data.starts_with(&[0xFE, 0xFF]) {
                (true, &data[2..])
            } else if data.starts_with(&[0xFF, 0xFE]) {
                (false, &data[2..])
            } else {
                (false, data)
            };
            let units = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| if big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
            utf16(units)
        }
        2 => utf16(data.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]]))),
        _ => String::from_utf8_lossy(data).into_owned(),
    };
    let s = match s.find('\0') {
        Some(nul) => &s[..nul],
        None => &s[..],
    };
    s.trim().to_string()
}

/// The position after a terminator for the given text encoding.
pub(crate) fn skip_id3_string(d: &[u8], pos: usize, encoding: u8) -> usize {
    if encoding == 1 || encoding == 2 {
        let mut i = pos;
        while i + 1 < d.len() {
            if d[i] == 0 && d[i + 1] == 0 {
                return i + 2;
            }
            i += 2;
        }
        return d.len();
    }
    match d.get(pos..).and_then(|rest| rest.iter().position(|&b| b == 0)) {
        Some(i) => pos + i + 1,
        None => d.len(),
    }
}

fn skip_ascii_space(s: &str) -> &str {
    s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
}

/// `^\s*(\d+)` then as an int: "3/12" -> 3, "2019-04-01" -> 2019; 0
/// when there is none or it overflows an int.
pub(crate) fn leading_int(s: &str) -> i32 {
    let s = skip_ascii_space(s);
    let digits: &str = &s[..s.bytes().take_while(|b| b.is_ascii_digit()).count()];
    digits.parse::<i32>().unwrap_or(0)
}

/// `^\s*([+-]?\d*\.?\d+(?:[eE][+-]?\d+)?)` then toDouble(): "-6.54 dB" ->
/// -6.54; NaN when there is none.
pub(crate) fn leading_number(s: &str) -> f64 {
    let s = skip_ascii_space(s);
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut end = i;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 {
            end = j;
        } else if int_digits == 0 {
            return f64::NAN;
        }
    } else if int_digits == 0 {
        return f64::NAN;
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut j = end + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            end = j;
        }
    }
    s[..end].parse::<f64>().unwrap_or(f64::NAN)
}

/// "(17)Rock" / "(17)" -> "Rock" / "".
pub(crate) fn strip_genre_code(g: &str) -> String {
    if let Some(rest) = g.strip_prefix('(') {
        let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0 && rest[digits..].starts_with(')') {
            return rest[digits + 1..].trim().to_string();
        }
    }
    g.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(leading_int("3/12"), 3);
        assert_eq!(leading_int(" 2019-04-01"), 2019);
        assert_eq!(leading_int("x1"), 0);
        assert_eq!(leading_int("99999999999"), 0);
        assert_eq!(leading_number("-6.54 dB"), -6.54);
        assert_eq!(leading_number("+0.3 dB"), 0.3);
        assert_eq!(leading_number("0.988"), 0.988);
        assert_eq!(leading_number("1."), 1.0);
        assert_eq!(leading_number(".5"), 0.5);
        assert_eq!(leading_number("2e3x"), 2000.0);
        assert!(leading_number("dB").is_nan());
        assert!(leading_number("-").is_nan());
        assert_eq!(strip_genre_code("(17)Rock"), "Rock");
        assert_eq!(strip_genre_code("(8)"), "");
        assert_eq!(strip_genre_code("Jazz"), "Jazz");
    }

    #[test]
    fn id3_text() {
        assert_eq!(decode_id3_text(b"Four\0Second", 3), "Four");
        assert_eq!(decode_id3_text(&[0xFF, 0xFE, b'h', 0, b'i', 0], 1), "hi");
        assert_eq!(decode_id3_text(&[0, b'h', 0, b'i'], 2), "hi");
        assert_eq!(decode_id3_text(&[0xE9], 0), "é");
        assert_eq!(skip_id3_string(b"\x01ab\0c", 1, 0), 4);
    }
}
