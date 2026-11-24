//! The settings file: an INI file in the stored format, written exactly as
//! earlier Sonkore versions wrote it (its escaping, string lists and value
//! texts), so they and this one share it.
//!
//! Keys are paths ("ui/compactMode"): the first part is the INI section,
//! the rest the key in it ('/' written as '\'). Values are kept as the format
//! stores them: text, lists, byte arrays (@ByteArray(...), the JSON blobs)
//! and anything else earlier versions wrote (@Variant(...), @Size(...))
//! untouched.
//! Saving re-reads the file and applies only what changed here, so other
//! writers' changes are not lost.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// A value as the file keeps it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Text (numbers and booleans are text too: "42", "true").
    Text(String),
    /// A list of two or more texts, or an empty one (@Invalid()).
    List(Vec<String>),
    /// @ByteArray(...): Sonkore keeps JSON documents this way.
    Bytes(Vec<u8>),
    /// Another stored type (@Variant(...), @Size(...), @Rect(...), @Point(...)):
    /// the unescaped text, written back as it was.
    Other(String),
}

impl Value {
    /// As text: a byte array as Latin-1.
    pub fn as_text(&self) -> String {
        match self {
            Value::Text(s) => s.clone(),
            Value::List(l) => l.first().cloned().unwrap_or_default(),
            Value::Bytes(b) => b.iter().map(|&c| c as char).collect(),
            Value::Other(_) => String::new(),
        }
    }

    /// As a bool, from text: false for "", "0" and "false".
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Text(s) => {
                let t = s.trim();
                Some(!(t.is_empty() || t == "0" || t.eq_ignore_ascii_case("false")))
            }
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Text(s) => {
                let t = s.trim();
                t.parse::<i64>()
                    .ok()
                    .or_else(|| t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.round() as i64))
            }
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Text(s) => s.trim().parse::<f64>().ok(),
            _ => None,
        }
    }

    /// As a list: a lone text is a list of one, an empty text
    /// none (Sonkore writes empty lists as "").
    pub fn as_list(&self) -> Vec<String> {
        match self {
            Value::List(l) => l.clone(),
            Value::Text(s) if s.is_empty() => Vec::new(),
            Value::Text(s) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

/// The settings: what the file held when read, plus the changes made since.
#[derive(Debug)]
pub struct Settings {
    path: PathBuf,
    values: BTreeMap<String, Value>,
    changed: BTreeSet<String>, // set or removed here since the last save
}

impl Settings {
    /// The settings file at `path` (missing or unreadable: empty).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let values = fs::read(&path).map(|bytes| parse(&bytes)).unwrap_or_default();
        Settings { path, values, changed: BTreeSet::new() }
    }

    /// Sonkore's own settings file (paths::settings_file()).
    pub fn open_default() -> Self {
        Self::open(crate::paths::settings_file())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn text(&self, key: &str, fallback: &str) -> String {
        self.value(key).map(Value::as_text).unwrap_or_else(|| fallback.to_owned())
    }

    pub fn flag(&self, key: &str, fallback: bool) -> bool {
        self.value(key).and_then(Value::as_bool).unwrap_or(fallback)
    }

    pub fn int(&self, key: &str, fallback: i64) -> i64 {
        self.value(key).and_then(Value::as_i64).unwrap_or(fallback)
    }

    pub fn number(&self, key: &str, fallback: f64) -> f64 {
        self.value(key).and_then(Value::as_f64).unwrap_or(fallback)
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        self.value(key).map(Value::as_list).unwrap_or_default()
    }

    pub fn bytes(&self, key: &str) -> Option<&[u8]> {
        self.value(key).and_then(Value::as_bytes)
    }

    /// The keys directly or deeply under `group` ("ui" → "ui/compactMode"…).
    pub fn keys_in(&self, group: &str) -> Vec<String> {
        let prefix = format!("{group}/");
        self.values.keys().filter(|k| k.starts_with(&prefix)).cloned().collect()
    }

    pub fn set(&mut self, key: &str, value: Value) {
        if self.values.get(key) == Some(&value) {
            return;
        }
        self.values.insert(key.to_owned(), value);
        self.changed.insert(key.to_owned());
    }

    pub fn set_text(&mut self, key: &str, value: impl Into<String>) {
        self.set(key, Value::Text(value.into()));
    }

    pub fn set_flag(&mut self, key: &str, value: bool) {
        self.set_text(key, if value { "true" } else { "false" });
    }

    pub fn set_int(&mut self, key: &str, value: i64) {
        self.set_text(key, value.to_string());
    }

    pub fn set_number(&mut self, key: &str, value: f64) {
        self.set_text(key, format_double(value));
    }

    /// The format writes a list of one as a lone text; reading gives it back.
    pub fn set_list(&mut self, key: &str, value: &[String]) {
        match value {
            [one] => self.set_text(key, one.clone()),
            _ => self.set(key, Value::List(value.to_vec())),
        }
    }

    pub fn set_bytes(&mut self, key: &str, value: impl Into<Vec<u8>>) {
        self.set(key, Value::Bytes(value.into()));
    }

    pub fn remove(&mut self, key: &str) {
        if self.values.remove(key).is_some() {
            self.changed.insert(key.to_owned());
        }
    }

    pub fn has_changes(&self) -> bool {
        !self.changed.is_empty()
    }

    /// Writes the changes: the file as it is now on disk, with them applied,
    /// replaced in one step.
    pub fn save(&mut self) -> io::Result<()> {
        if self.changed.is_empty() {
            return Ok(());
        }
        // Only a missing file is an empty one: a file that cannot be read now
        // is left as it is (the changes stay for the next save).
        let mut merged = match fs::read(&self.path) {
            Ok(bytes) => parse(&bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        for key in &self.changed {
            match self.values.get(key) {
                Some(value) => merged.insert(key.clone(), value.clone()),
                None => merged.remove(key),
            };
        }
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temporary = self.path.with_extension("conf.tmp");
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&write(&merged))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &self.path)?;
        self.values = merged;
        self.changed.clear();
        Ok(())
    }
}

// --- Reading ---------------------------------------------------------------------

/// The keys and values of an INI file.
pub fn parse(bytes: &[u8]) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    let mut section = String::new();
    for line in logical_lines(bytes) {
        let trimmed = trim_ascii(&line);
        if trimmed.is_empty() || trimmed[0] == b';' || trimmed[0] == b'#' {
            continue;
        }
        if trimmed[0] == b'[' {
            let end = trimmed.iter().position(|&c| c == b']').unwrap_or(trimmed.len());
            let name = unescape_key(&trimmed[1..end]);
            section = if name.eq_ignore_ascii_case("general") { String::new() } else { name };
            continue;
        }
        let Some(eq) = trimmed.iter().position(|&c| c == b'=') else { continue };
        let key = unescape_key(trim_ascii(&trimmed[..eq]));
        if key.is_empty() {
            continue;
        }
        let full = if section.is_empty() { key } else { format!("{section}/{key}") };
        values.insert(full, parse_value(strip_comment(trim_ascii(&trimmed[eq + 1..]))));
    }
    values
}

/// Lines, a backslash at the end of one joining it to the next.
fn logical_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' || c == b'\r' {
            // A continued line keeps its backslash and line break for the
            // value's unescaping, which drops both.
            let continued = current.last() == Some(&b'\\') && !ends_escaped_backslash(&current);
            if continued {
                current.push(c);
            } else {
                lines.push(std::mem::take(&mut current));
            }
            if c == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                if continued {
                    current.push(b'\n');
                }
                i += 1;
            }
        } else {
            current.push(c);
        }
        i += 1;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn ends_escaped_backslash(line: &[u8]) -> bool {
    let n = line.iter().rev().take_while(|&&c| c == b'\\').count();
    n % 2 == 0
}

fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(start, |e| e + 1);
    &s[start..end]
}

/// A ';' outside quotes starts a comment.
fn strip_comment(s: &[u8]) -> &[u8] {
    let mut quoted = false;
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'\\' => i += 1,
            b'"' => quoted = !quoted,
            b';' if !quoted => return trim_ascii(&s[..i]),
            _ => {}
        }
        i += 1;
    }
    s
}

/// iniUnescapedKey: %XX and %UXXXX escapes, '\' for '/'.
fn unescape_key(s: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'\\' {
            out.push('/');
            i += 1;
        } else if c == b'%' && i + 1 < s.len() {
            let (digits, start) = if s[i + 1] == b'U' { (4, i + 2) } else { (2, i + 1) };
            let hex = s
                .get(start..start + digits)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u32::from_str_radix(h, 16).ok());
            match hex.and_then(char::from_u32) {
                Some(ch) => {
                    out.push(ch);
                    i = start + digits;
                }
                None => {
                    out.push('%');
                    i += 1;
                }
            }
        } else {
            // Raw UTF-8 runs.
            let end = s[i..].iter().position(|&b| b == b'\\' || b == b'%').map_or(s.len(), |p| i + p);
            out.push_str(&String::from_utf8_lossy(&s[i..end]));
            i = end;
        }
    }
    out
}

/// iniUnescapedStringList, then stringToVariant.
fn parse_value(s: &[u8]) -> Value {
    let (text, list) = unescape_value(s);
    match list {
        Some(items) => Value::List(items.into_iter().map(|item| unmark(&item)).collect()),
        None => string_to_value(text),
    }
}

/// The value's text, or its items when it is a list (unquoted commas).
fn unescape_value(s: &[u8]) -> (String, Option<Vec<String>>) {
    let mut items: Option<Vec<String>> = None;
    let mut current: Vec<u16> = Vec::new();
    let mut quoted_value = false; // the current item had quotes: its spaces stay
    let mut in_quotes = false;
    let mut chop_limit = 0usize; // escapes are never trimmed away
    let mut i = 0;
    let skip_spaces = |i: &mut usize| {
        while *i < s.len() && (s[*i] == b' ' || s[*i] == b'\t') {
            *i += 1;
        }
    };
    let chop = |current: &mut Vec<u16>, limit: usize| {
        while current.len() > limit && matches!(current.last(), Some(&c) if c == b' ' as u16 || c == b'\t' as u16) {
            current.pop();
        }
    };
    skip_spaces(&mut i);
    while i < s.len() {
        match s[i] {
            b'\\' => {
                i += 1;
                let Some(&c) = s.get(i) else { break };
                i += 1;
                let simple = match c {
                    b'a' => Some(0x07),
                    b'b' => Some(0x08),
                    b'f' => Some(0x0c),
                    b'n' => Some(b'\n' as u16),
                    b'r' => Some(b'\r' as u16),
                    b't' => Some(b'\t' as u16),
                    b'v' => Some(0x0b),
                    b'"' | b'?' | b'\'' | b'\\' => Some(c as u16),
                    _ => None,
                };
                if let Some(code) = simple {
                    current.push(code);
                } else if c == b'x' {
                    let start = i;
                    while i < s.len() && s[i].is_ascii_hexdigit() {
                        i += 1;
                    }
                    if i > start {
                        let digits = std::str::from_utf8(&s[start..i]).unwrap_or("0");
                        current.push(u32::from_str_radix(digits, 16).unwrap_or(0) as u16);
                    }
                } else if (b'0'..=b'7').contains(&c) {
                    let mut v = (c - b'0') as u32;
                    while i < s.len() && (b'0'..=b'7').contains(&s[i]) {
                        v = v * 8 + (s[i] - b'0') as u32;
                        i += 1;
                    }
                    current.push(v as u16);
                } else if c == b'\n' || c == b'\r' {
                    // A line continuation.
                    if let Some(&c2) = s.get(i) {
                        if (c2 == b'\n' || c2 == b'\r') && c2 != c {
                            i += 1;
                        }
                    }
                }
                chop_limit = current.len();
            }
            b'"' => {
                i += 1;
                quoted_value = true;
                in_quotes = !in_quotes;
                if !in_quotes {
                    skip_spaces(&mut i);
                }
            }
            b',' if !in_quotes => {
                if !quoted_value {
                    chop(&mut current, chop_limit);
                }
                items.get_or_insert_with(Vec::new).push(String::from_utf16_lossy(&current));
                current.clear();
                quoted_value = false;
                chop_limit = 0;
                i += 1;
                skip_spaces(&mut i);
            }
            _ => {
                let start = i;
                while i < s.len() && !matches!(s[i], b'\\' | b'"' | b',') {
                    i += 1;
                }
                let run = String::from_utf8_lossy(&s[start..i]);
                current.extend(run.encode_utf16());
                // A comma inside quotes is plain text.
                if i < s.len() && s[i] == b',' && in_quotes {
                    current.push(b',' as u16);
                    i += 1;
                }
            }
        }
    }
    if !quoted_value {
        chop(&mut current, chop_limit);
    }
    let text = String::from_utf16_lossy(&current);
    match items {
        Some(mut list) => {
            list.push(text);
            (String::new(), Some(list))
        }
        None => (text, None),
    }
}

/// stringToVariant for one item of a list: "@@x" is the text "@x".
fn unmark(item: &str) -> String {
    match item.strip_prefix('@') {
        Some(rest) if rest.starts_with('@') => rest.to_owned(),
        _ => item.to_owned(),
    }
}

/// stringToVariant.
fn string_to_value(text: String) -> Value {
    if !text.starts_with('@') {
        return Value::Text(text);
    }
    if let Some(rest) = text.strip_prefix("@@") {
        return Value::Text(format!("@{rest}"));
    }
    if text == "@Invalid()" {
        return Value::List(Vec::new());
    }
    if let Some(inner) = text.strip_prefix("@ByteArray(").and_then(|t| t.strip_suffix(')')) {
        // Each character is one byte (Latin-1).
        return Value::Bytes(inner.chars().map(|c| c as u32 as u8).collect());
    }
    if text.starts_with("@Variant(")
        || text.starts_with("@Size(")
        || text.starts_with("@Rect(")
        || text.starts_with("@Point(")
    {
        return Value::Other(text);
    }
    Value::Text(text)
}

// --- Writing ---------------------------------------------------------------------

/// The file's text: [General] first for keys outside any group, then the
/// sections and their keys in order (sorted, as the stored format has them).
pub fn write(values: &BTreeMap<String, Value>) -> Vec<u8> {
    let mut sections: BTreeMap<&str, Vec<(&str, &Value)>> = BTreeMap::new();
    for (key, value) in values {
        let (section, name) = match key.split_once('/') {
            Some((section, name)) => (section, name),
            None => ("", key.as_str()),
        };
        sections.entry(section).or_default().push((name, value));
    }
    let mut out = Vec::new();
    for (i, (section, keys)) in sections.iter().enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.push(b'[');
        if section.is_empty() {
            out.extend_from_slice(b"General");
        } else if section.eq_ignore_ascii_case("general") {
            out.push(b'%');
            escape_key(section, &mut out);
        } else {
            escape_key(section, &mut out);
        }
        out.extend_from_slice(b"]\n");
        for (name, value) in keys {
            escape_key(name, &mut out);
            out.push(b'=');
            write_value(value, &mut out);
            out.push(b'\n');
        }
    }
    out
}

/// iniEscapedKey.
fn escape_key(key: &str, out: &mut Vec<u8>) {
    for ch in key.chars() {
        match ch {
            '/' => out.push(b'\\'),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' => out.push(ch as u8),
            _ => {
                for unit in ch.encode_utf16(&mut [0; 2]) {
                    let unit = *unit as u32;
                    if unit <= 0xff {
                        out.extend_from_slice(format!("%{unit:02X}").as_bytes());
                    } else {
                        out.extend_from_slice(format!("%U{unit:04X}").as_bytes());
                    }
                }
            }
        }
    }
}

fn write_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Text(s) => escape_string(&mark(s), out),
        Value::List(items) if items.is_empty() => out.extend_from_slice(b"@Invalid()"),
        Value::List(items) => {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.extend_from_slice(b", ");
                }
                escape_string(&mark(item), out);
            }
        }
        Value::Bytes(bytes) => {
            let mut s = String::from("@ByteArray(");
            s.extend(bytes.iter().map(|&b| b as char));
            s.push(')');
            escape_string(&s, out);
        }
        Value::Other(s) => escape_string(s, out),
    }
}

/// variantToString on text: a leading '@' doubled.
fn mark(s: &str) -> String {
    if s.starts_with('@') { format!("@{s}") } else { s.to_owned() }
}

/// iniEscapedString: escapes, UTF-8 for text (byte arrays and variants
/// escape everything past ASCII), quotes when needed.
fn escape_string(s: &str, out: &mut Vec<u8>) {
    let raw_bytes = s.starts_with("@ByteArray(") || s.starts_with("@Variant(");
    let start = out.len();
    let mut needs_quotes = false;
    let mut escape_next_if_digit = false;
    let mut utf8 = [0u8; 4];
    for unit in s.encode_utf16() {
        let ch = unit as u32;
        if ch == ';' as u32 || ch == ',' as u32 || ch == '=' as u32 {
            needs_quotes = true;
        }
        let is_hex = |c: u32| char::from_u32(c).is_some_and(|c| c.is_ascii_hexdigit());
        if escape_next_if_digit && is_hex(ch) {
            out.extend_from_slice(format!("\\x{ch:x}").as_bytes());
            continue;
        }
        escape_next_if_digit = false;
        match ch {
            0 => {
                out.extend_from_slice(b"\\0");
                escape_next_if_digit = true;
            }
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x22 | 0x5c => {
                out.push(b'\\');
                out.push(ch as u8);
            }
            _ if ch <= 0x1f || (ch >= 0x7f && raw_bytes) => {
                out.extend_from_slice(format!("\\x{ch:x}").as_bytes());
                escape_next_if_digit = true;
            }
            _ if ch < 0x80 => out.push(ch as u8),
            _ => {
                // Text past ASCII as UTF-8 (surrogate pairs rejoined below).
                if let Some(c) = char::from_u32(ch) {
                    out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
                }
            }
        }
    }
    // Surrogate pairs: re-encode the whole text when it has any (rare).
    if s.chars().any(|c| c as u32 > 0xffff) && !raw_bytes {
        out.truncate(start);
        let mut fixed = Vec::new();
        for c in s.chars() {
            if c as u32 > 0xffff {
                fixed.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
            } else {
                let mut one = Vec::new();
                escape_string_bmp(c, &mut one);
                fixed.extend(one);
            }
        }
        out.extend(fixed);
    }
    let body = &out[start..];
    if needs_quotes || body.first() == Some(&b' ') || body.last() == Some(&b' ') {
        out.insert(start, b'"');
        out.push(b'"');
    }
}

fn escape_string_bmp(c: char, out: &mut Vec<u8>) {
    let mut buf = [0u8; 4];
    match c {
        '\0' => out.extend_from_slice(b"\\0"),
        '\x07' => out.extend_from_slice(b"\\a"),
        '\x08' => out.extend_from_slice(b"\\b"),
        '\x0c' => out.extend_from_slice(b"\\f"),
        '\n' => out.extend_from_slice(b"\\n"),
        '\r' => out.extend_from_slice(b"\\r"),
        '\t' => out.extend_from_slice(b"\\t"),
        '\x0b' => out.extend_from_slice(b"\\v"),
        '"' | '\\' => {
            out.push(b'\\');
            out.push(c as u8);
        }
        c if (c as u32) <= 0x1f => out.extend_from_slice(format!("\\x{:x}", c as u32).as_bytes()),
        c => out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
    }
}

/// A double as the format writes it: the shortest text that reads back the
/// same, in C locale ("1", "0.5", "1.41421356237", "1e-05").
pub fn format_double(v: f64) -> String {
    if !v.is_finite() {
        return if v.is_nan() {
            "nan".into()
        } else if v > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    if v == v.trunc() && v.abs() < 1e17 {
        return format!("{}", v as i64);
    }
    // Rust's shortest digits, laid out as printf's %g would.
    let sci = format!("{v:e}"); // "1.4142e0", "-1e-5"
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits = mantissa.trim_start_matches('-').replace('.', "").len() as i32;
    if exponent < -4 || exponent >= digits.max(17) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br#"[appearance]
accentOverride=
alternateRows=false
skinIndex=0
windowRadius=8

[equalizer]
bands="@ByteArray([{\"enabled\":true,\"freq\":31.25,\"q\":1.41421356237,\"type\":\"PK\"}])"

[ui]
compactColumns=fileName, format, duration
lastFolder=/home/pc/M\xfasica
title="  spaced, and; odd  "
at=@@home
size=@Size(480 460)
empty=@Invalid()
"#;

    /// A file that is there but cannot be read is not replaced by one with
    /// only the keys changed here.
    #[test]
    #[cfg(unix)]
    fn unreadable_file_is_not_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sonkore-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sonkore.conf");
        fs::write(&path, "[a]\nx=1\ny=2\n").unwrap();
        let mut s = Settings::open(&path);
        s.set_int("a/x", 3);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = fs::read(&path).is_err(); // root reads it anyway
        let saved = s.save();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        if unreadable {
            assert!(saved.is_err());
            assert!(s.has_changes(), "kept for the next save");
            assert_eq!(fs::read_to_string(&path).unwrap(), "[a]\nx=1\ny=2\n");
        }
        // Readable again: saved, the other keys kept.
        s.save().unwrap();
        let v = parse(&fs::read(&path).unwrap());
        assert_eq!(v["a/x"], Value::Text("3".into()));
        assert_eq!(v["a/y"], Value::Text("2".into()));
        // A missing file is an empty one.
        fs::remove_file(&path).unwrap();
        let mut s = Settings::open(&path);
        s.set_int("a/z", 4);
        s.save().unwrap();
        assert_eq!(parse(&fs::read(&path).unwrap())["a/z"], Value::Text("4".into()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_stored_values() {
        let v = parse(SAMPLE);
        assert_eq!(v["appearance/accentOverride"], Value::Text(String::new()));
        assert_eq!(v["appearance/alternateRows"].as_bool(), Some(false));
        assert_eq!(v["appearance/windowRadius"].as_i64(), Some(8));
        let bands = v["equalizer/bands"].as_bytes().unwrap();
        assert_eq!(
            std::str::from_utf8(bands).unwrap(),
            r#"[{"enabled":true,"freq":31.25,"q":1.41421356237,"type":"PK"}]"#
        );
        assert_eq!(v["ui/compactColumns"].as_list(), ["fileName", "format", "duration"]);
        assert_eq!(v["ui/lastFolder"].as_text(), "/home/pc/Música");
        assert_eq!(v["ui/title"].as_text(), "  spaced, and; odd  ");
        assert_eq!(v["ui/at"].as_text(), "@home");
        assert_eq!(v["ui/size"], Value::Other("@Size(480 460)".into()));
        assert_eq!(v["ui/empty"].as_list(), Vec::<String>::new());
    }

    #[test]
    fn writes_what_it_reads() {
        let v = parse(SAMPLE);
        let again = parse(&write(&v));
        assert_eq!(v, again);
    }

    #[test]
    fn writes_compatibly() {
        let mut v = BTreeMap::new();
        v.insert("ui/lastFolder".to_string(), Value::Text("/home/pc/Música".into()));
        v.insert("ui/title".to_string(), Value::Text("a, b".into()));
        v.insert("ui/cols".to_string(), Value::List(vec!["x".into(), "y".into()]));
        v.insert("eq/bands".to_string(), Value::Bytes(br#"{"a":1}"#.to_vec()));
        v.insert("eq/list".to_string(), Value::Bytes(br#"[1,2]"#.to_vec()));
        v.insert("eq/deep/key".to_string(), Value::Text("1".into()));
        v.insert("top".to_string(), Value::Text("@x".into()));
        let text = String::from_utf8(write(&v)).unwrap();
        assert_eq!(
            text,
            "[General]\ntop=@@x\n\n[eq]\nbands=@ByteArray({\\\"a\\\":1})\ndeep\\key=1\nlist=\"@ByteArray([1,2])\"\n\n[ui]\ncols=x, y\nlastFolder=/home/pc/Música\ntitle=\"a, b\"\n"
        );
    }

    #[test]
    #[allow(clippy::approx_constant)] // a stored value, not √2
    fn doubles_formatted_as_stored() {
        assert_eq!(format_double(1.0), "1");
        assert_eq!(format_double(0.5), "0.5");
        assert_eq!(format_double(1.41421356237), "1.41421356237");
        assert_eq!(format_double(-0.317666), "-0.317666");
        assert_eq!(format_double(0.00001), "1e-05");
    }
}
