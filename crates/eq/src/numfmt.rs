//! The stored formats of EQ profiles and settings, as earlier Sonkore
//! versions wrote them: compact and indented JSON, loose conversions of
//! JSON values, and numbers as `%g` / `%f` with a given precision.

use serde_json::Value;
use sonkore_core::settings::format_double;

/// JSON text: Compact (`{"a":1}`) or Indented (four spaces,
/// `"key": value`, a final newline). Object keys come out sorted.
pub fn to_json(value: &Value, compact: bool) -> String {
    let mut out = String::new();
    match value {
        Value::Array(_) | Value::Object(_) => {
            write_value(value, &mut out, 0, compact);
            if !compact {
                out.push('\n');
            }
        }
        other => write_value(other, &mut out, 0, compact),
    }
    out
}

fn write_value(value: &Value, out: &mut String, indent: usize, compact: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => out.push_str(&i.to_string()),
            (_, Some(u), _) => out.push_str(&u.to_string()),
            (_, _, Some(f)) if f.is_finite() => out.push_str(&format_double(f)),
            _ => out.push_str("null"), // +INF, -INF, NaN (RFC 4627 §2.4)
        },
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push_str(if compact { "[" } else { "[\n" });
            let inner = indent + usize::from(!compact);
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(if compact { "," } else { ",\n" });
                }
                push_indent(out, inner, compact);
                write_value(item, out, inner, compact);
            }
            if !items.is_empty() && !compact {
                out.push('\n');
            }
            push_indent(out, indent, compact);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str(if compact { "{" } else { "{\n" });
            let inner = indent + usize::from(!compact);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(if compact { "," } else { ",\n" });
                }
                push_indent(out, inner, compact);
                write_string(key, out);
                out.push_str(if compact { ":" } else { ": " });
                write_value(&map[*key], out, inner, compact);
            }
            if !map.is_empty() && !compact {
                out.push('\n');
            }
            push_indent(out, indent, compact);
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, indent: usize, compact: bool) {
    if !compact {
        out.extend(std::iter::repeat_n(' ', 4 * indent));
    }
}

/// The escaping: `"` `\` and the C escapes; other control characters as
/// \u00xx; everything else (non-ASCII included) as UTF-8.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// --- conversions of JSON values ---------------------------------------------------

/// A JSON value as a number (0 when it is not a number).
pub fn to_double(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => parse_double(s),
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

/// A JSON value as a flag.
pub fn to_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => {
            let t = s.trim();
            !(t.is_empty() || t == "0" || t.eq_ignore_ascii_case("false"))
        }
        _ => false,
    }
}

/// A JSON value as text.
pub fn to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => n.as_f64().map(format_double).unwrap_or_default(),
        },
        _ => String::new(),
    }
}

/// A number from text: C locale, surrounding spaces allowed, 0 when the
/// text is not a finite number ("inf", "nan", or digits past f64's range:
/// an infinite gain made NaN coefficients and a NaN output).
pub fn parse_double(s: &str) -> f64 {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)
}

// --- numbers as text ---------------------------------------------------------------

/// A double's exact decimal digits (no leading or trailing zeros) and the
/// position of the decimal point: |v| = 0.d1d2d3… × 10^point.
fn exact_decimal(v: f64) -> (Vec<u8>, i32) {
    // 767 significant digits are enough for any double's exact expansion.
    let s = format!("{:.800e}", v.abs());
    let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
    let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).map(|c| c - b'0').collect();
    while digits.last() == Some(&0) {
        digits.pop();
    }
    (digits, exponent.parse::<i32>().unwrap_or(0) + 1)
}

/// Keeps the first `keep` digits (`keep` may be ≤ 0), rounding half away
/// from zero; the point moves on a carry.
fn round_digits(digits: &mut Vec<u8>, point: &mut i32, keep: i32) {
    if keep < 0 {
        digits.clear();
        return;
    }
    let keep = keep as usize;
    if digits.len() <= keep {
        return;
    }
    let up = digits[keep] >= 5;
    digits.truncate(keep);
    if up {
        let mut i = keep;
        loop {
            if i == 0 {
                digits.insert(0, 1);
                *point += 1;
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
}

fn digit_at(digits: &[u8], i: i32) -> char {
    if i >= 0 && (i as usize) < digits.len() { char::from(b'0' + digits[i as usize]) } else { '0' }
}

/// The digits laid out with `decimals` places after the point.
fn fixed_layout(digits: &[u8], point: i32, decimals: usize, negative: bool) -> String {
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if point <= 0 {
        out.push('0');
    } else {
        for i in 0..point {
            out.push(digit_at(digits, i));
        }
    }
    if decimals > 0 {
        out.push('.');
        for i in 0..decimals as i32 {
            out.push(digit_at(digits, point + i));
        }
    }
    out
}

fn non_finite(v: f64) -> Option<String> {
    (!v.is_finite()).then(|| {
        if v.is_nan() {
            "nan".into()
        } else if v > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        }
    })
}

/// `%f`: `precision` decimals, ties rounded
/// away from zero; a negative value keeps its sign even when it rounds to
/// zero ("-0.0"), -0.0 itself has none.
pub fn number_f(v: f64, precision: usize) -> String {
    if let Some(s) = non_finite(v) {
        return s;
    }
    let (mut digits, mut point) = exact_decimal(v);
    let keep = point + precision as i32;
    round_digits(&mut digits, &mut point, keep);
    if digits.is_empty() {
        point = 0;
    }
    fixed_layout(&digits, point, precision, v < 0.0)
}

/// `%g`: printf's %g (`precision`
/// significant digits, trailing zeros dropped, the exponent form below
/// 1e-4 or from 10^precision, as "1.5e+06"), ties rounded away from zero.
pub fn number_g(v: f64, precision: usize) -> String {
    if let Some(s) = non_finite(v) {
        return s;
    }
    let p = precision.max(1) as i32;
    let negative = v < 0.0; // -0.0 has no sign, -0.04 rounded to "-0" keeps it
    let (mut digits, mut point) = exact_decimal(v);
    if digits.is_empty() {
        return "0".into();
    }
    round_digits(&mut digits, &mut point, p);
    let x = point - 1;
    if x < -4 || x >= p {
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push(digit_at(&digits, 0));
        if digits.len() > 1 {
            out.push('.');
            for i in 1..digits.len() as i32 {
                out.push(digit_at(&digits, i));
            }
        }
        out.push_str(&format!("e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs()));
        out
    } else {
        let decimals = (digits.len() as i32 - point).max(0) as usize;
        fixed_layout(&digits, point, decimals, negative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn g_format() {
        assert_eq!(number_g(31.25, 6), "31.25");
        assert_eq!(number_g(1000.0, 6), "1000");
        assert_eq!(number_g(123456.0, 6), "123456");
        assert_eq!(number_g(1234567.0, 6), "1.23457e+06");
        assert_eq!(number_g(0.0001, 6), "0.0001");
        assert_eq!(number_g(0.00001234, 3), "1.23e-05");
        assert_eq!(number_g(1.5, 3), "1.5");
        assert_eq!(number_g(12.5, 3), "12.5");
        assert_eq!(number_g(999.95, 4), "1000");
        assert_eq!(number_g(99999.0, 4), "1e+05");
        assert_eq!(number_g(0.0, 4), "0");
        assert_eq!(number_g(10000.25, 6), "10000.3");
        assert_eq!(number_g(-2.5, 1), "-3");
        assert_eq!(number_f(-0.04, 1), "-0.0");
        assert_eq!(number_f(0.70710, 2), "0.71");
        assert_eq!(number_f(-4.25, 1), "-4.3");
        assert_eq!(number_f(0.125, 2), "0.13");
        assert_eq!(number_f(0.995, 2), "0.99"); // 0.99499999…
        assert_eq!(number_f(9.96, 1), "10.0");
        assert_eq!(number_f(0.0, 1), "0.0");
        assert_eq!(number_f(12.0, 0), "12");
    }

    #[test]
    fn json_layouts() {
        let v = json!([{"name": "A", "bands": [], "preamp": -3.5, "z": {"q": 1.41421356237}}]);
        assert_eq!(to_json(&v, true), r#"[{"bands":[],"name":"A","preamp":-3.5,"z":{"q":1.41421356237}}]"#);
        assert_eq!(
            to_json(&v, false),
            "[\n    {\n        \"bands\": [\n        ],\n        \"name\": \"A\",\n        \"preamp\": -3.5,\n        \"z\": {\n            \"q\": 1.41421356237\n        }\n    }\n]\n"
        );
        assert_eq!(to_json(&json!([]), false), "[\n]\n");
        assert_eq!(to_json(&json!(["a\"\u{1}é"]), true), "[\"a\\\"\\u0001é\"]");
    }
}
