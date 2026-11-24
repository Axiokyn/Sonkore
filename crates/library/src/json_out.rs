//! JSON written byte for byte in the stored format (as earlier Sonkore
//! versions wrote it), so a file saved here is the file they would have saved
//! (and a diff between the two shows real differences only). Reading goes
//! through serde_json.
//!
//! The format:
//! - object keys sorted by UTF-16 code units;
//! - a double that is a whole number within ±2^53 is written as an integer
//!   ("3", "1795291384044"), -0 as "0";
//! - other doubles in the shortest form that reads back exactly: decimal or
//!   exponent ("1e-05", "1e+16"), whichever is shorter, decimal on a tie;
//! - NaN and infinities as null;
//! - strings as UTF-8, escaping `"` `\` \b \f \n \r \t and the other control
//!   characters below 0x20 as \u00xx (lower-case hex); '/' and DEL as they are;
//! - Indented: four spaces a level, `"key": value`, and an empty array or
//!   object still opened and closed on two lines ("[\n    ]").

use serde_json::Value;

/// Writes `value` in the compact form.
pub fn to_compact(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, None);
    out
}

/// Writes `value` in the indented form (with the final newline).
pub fn to_indented(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, Some(0));
    out.push('\n');
    out
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("    ");
    }
}

fn write_value(out: &mut String, value: &Value, level: Option<usize>) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                // Integers are kept as i64; larger ones become doubles.
                write_double(out, u as f64);
            } else {
                write_double(out, n.as_f64().unwrap_or(f64::NAN));
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            match level {
                None => {
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        write_value(out, item, None);
                    }
                }
                Some(l) => {
                    out.push('\n');
                    for (i, item) in items.iter().enumerate() {
                        indent(out, l + 1);
                        write_value(out, item, Some(l + 1));
                        out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                    }
                    indent(out, l);
                }
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            match level {
                None => {
                    for (i, key) in keys.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        write_string(out, key);
                        out.push(':');
                        write_value(out, &map[*key], None);
                    }
                }
                Some(l) => {
                    out.push('\n');
                    for (i, key) in keys.iter().enumerate() {
                        indent(out, l + 1);
                        write_string(out, key);
                        out.push_str(": ");
                        write_value(out, &map[*key], Some(l + 1));
                        out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
                    }
                    indent(out, l);
                }
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
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

/// A double in the stored format (see the module notes).
pub fn write_double(out: &mut String, v: f64) {
    out.push_str(&format_double(v));
}

/// A double's text in the stored format: the shortest form, 'g' style.
pub fn format_double(v: f64) -> String {
    if !v.is_finite() {
        return "null".into();
    }
    const LIMIT: f64 = 9007199254740992.0; // 2^53
    if v == v.trunc() && v.abs() <= LIMIT {
        return (v as i64).to_string();
    }
    // Shortest round-trip digits and the decimal exponent, from Rust's
    // exponent form ("1.5e-7", "3.030204081632653e0").
    let sci = format!("{:e}", v.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let count = digits.len() as i32;
    let decpt = exp + 1; // value = 0.digits × 10^decpt
    let exp_digits = if exp.abs() >= 100 { 3 } else { 2 };
    let exp_len = count + i32::from(count > 1) + 2 + exp_digits;
    let use_decimal = if decpt <= 0 {
        2 - decpt + count <= exp_len
    } else if decpt <= count {
        true
    } else {
        decpt <= exp_len
    };
    let mut out = String::new();
    if v < 0.0 {
        out.push('-');
    }
    if use_decimal {
        if decpt <= 0 {
            out.push_str("0.");
            for _ in 0..(-decpt) {
                out.push('0');
            }
            out.push_str(digits);
        } else if decpt >= count {
            out.push_str(digits);
            for _ in 0..(decpt - count) {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..decpt as usize]);
            out.push('.');
            out.push_str(&digits[decpt as usize..]);
        }
    } else {
        out.push_str(&digits[..1]);
        if count > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let e = exp.unsigned_abs();
        if e < 10 {
            out.push('0');
        }
        out.push_str(&e.to_string());
    }
    out
}

/// serde_json's number for a double, kept as written by `format_double`
/// (whole numbers become integers, as in the stored format).
pub fn number(v: f64) -> Value {
    if v.is_finite() && v == v.trunc() && v.abs() <= 9007199254740992.0 {
        Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
    }
}

// --- Reading, with the stored format's conversions -----------------------------

/// Text, else "".
pub fn as_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A number, else `fallback`.
pub fn as_f64_or(v: Option<&Value>, fallback: f64) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(fallback),
        _ => fallback,
    }
}

/// An integer: a whole double converts, others give
/// the fallback.
pub fn as_i64_or(v: Option<&Value>, fallback: i64) -> i64 {
    match v {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                return i;
            }
            match n.as_f64() {
                Some(d) if d == d.trunc() && d.abs() < 9.2e18 => d as i64,
                _ => fallback,
            }
        }
        _ => fallback,
    }
}

/// As `as_i64_or` with 0, and 0 outside the int range.
pub fn as_i32(v: Option<&Value>) -> i32 {
    let i = as_i64_or(v, 0);
    i32::try_from(i).unwrap_or(0)
}

/// True only for `true`.
pub fn as_bool(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// A JSON array of texts (non-texts read as "", as in `as_string`).
pub fn string_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items.iter().map(|i| as_string(Some(i))).collect(),
        _ => Vec::new(),
    }
}

/// A list of texts as a JSON array.
pub fn array_of(list: &[String]) -> Value {
    Value::Array(list.iter().map(|s| Value::String(s.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_as_stored() {
        // Expected values in the stored format (see the module notes).
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (3.0, "3"),
            (1e6, "1000000"),
            (1e15, "1000000000000000"),
            (1e16, "1e+16"),
            (1e17, "1e+17"),
            (1e21, "1e+21"),
            (0.1, "0.1"),
            (1e-5, "1e-05"),
            (0.0001, "1e-04"),
            (1.5e-7, "1.5e-07"),
            (3.030204081632653, "3.030204081632653"),
            (-6.54, "-6.54"),
            (2.506666666666667, "2.506666666666667"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (1795291384044.0, "1795291384044"),
            (9007199254740993.0, "9007199254740992"),
            (12345678.9, "12345678.9"),
            (0.00030000000000000003, "0.00030000000000000003"),
            (f64::NAN, "null"),
        ];
        for (v, want) in cases {
            assert_eq!(format_double(v), want, "{v}");
        }
    }

    #[test]
    fn layout_as_stored() {
        let v: Value = serde_json::json!({"z": 1, "B": true, "empty": [], "eo": {}, "a": [1, "q\"\\/\u{8}\u{c}\n\r\t\u{1}\u{1f}\u{7f} é"]});
        assert_eq!(
            to_compact(&v),
            "{\"B\":true,\"a\":[1,\"q\\\"\\\\/\\b\\f\\n\\r\\t\\u0001\\u001f\u{7f} é\"],\"empty\":[],\"eo\":{},\"z\":1}"
        );
        assert_eq!(
            to_indented(&v),
            "{\n    \"B\": true,\n    \"a\": [\n        1,\n        \"q\\\"\\\\/\\b\\f\\n\\r\\t\\u0001\\u001f\u{7f} é\"\n    ],\n    \"empty\": [\n    ],\n    \"eo\": {\n    },\n    \"z\": 1\n}\n"
        );
    }
}
