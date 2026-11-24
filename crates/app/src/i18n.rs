//! The translations (i18n/<lang>.po) for the text made in Rust; Slint's @tr
//! texts use the same files, bundled by build.rs. Placeholders are Slint's:
//! {0}, {1}…, with literal braces doubled.

use std::collections::HashMap;
use std::sync::OnceLock;

static TABLE: OnceLock<HashMap<(String, String), String>> = OnceLock::new();
static LANG: OnceLock<String> = OnceLock::new();

const FILES: [(&str, &str); 5] = [
    ("es", include_str!("../../../i18n/es.po")),
    ("de", include_str!("../../../i18n/de.po")),
    ("fr", include_str!("../../../i18n/fr.po")),
    ("it", include_str!("../../../i18n/it.po")),
    ("pt_BR", include_str!("../../../i18n/pt_BR.po")),
];

/// The language to show: appearance/language, else the system's ($LANG…).
pub fn language(setting: &str) -> String {
    if !setting.is_empty() {
        return setting.to_owned();
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.split(['.', '@']).next().unwrap_or("").to_owned();
            if !v.is_empty() && v != "C" && v != "POSIX" {
                return v;
            }
        }
    }
    "en".into()
}

/// The file for `lang` ("es_AR" falls back to "es", "pt" to "pt_BR").
pub fn file_language(lang: &str) -> Option<&'static str> {
    FILES.iter().map(|(l, _)| *l).find(|l| *l == lang).or_else(|| {
        let base = lang.split('_').next().unwrap_or(lang);
        FILES.iter().map(|(l, _)| *l).find(|l| l.split('_').next() == Some(base))
    })
}

/// Loads the table for `lang` (once, at start-up).
pub fn load(lang: &str) {
    let _ = LANG.set(lang.to_owned());
    let table = file_language(lang)
        .and_then(|l| FILES.iter().find(|(f, _)| *f == l))
        .map(|(_, ts)| parse(ts))
        .unwrap_or_default();
    let _ = TABLE.set(table);
}

/// The language this run shows ("en" when none was loaded).
pub fn loaded() -> &'static str {
    LANG.get().and_then(|l| file_language(l)).unwrap_or("en")
}

/// Whether appearance/language set to `setting` shows another language
/// than this run's (the change waits for a restart: much of Rust's text is
/// made once, at start-up).
pub fn differs_from_loaded(setting: &str) -> bool {
    file_language(&language(setting)).unwrap_or("en") != loaded()
}

/// Slint's texts (@tr) in the loaded language. Call it after the first
/// component is created: before that Slint has no bundle to pick from
/// (NoTranslationsBundled) and keeps the system's language.
pub fn select_slint() {
    let lang = LANG.get().map(String::as_str).unwrap_or("en");
    let result = match file_language(lang) {
        Some(file) => slint::select_bundled_translation(file),
        // English: the texts as written.
        None => slint::select_bundled_translation(""),
    };
    if let Err(e) = result {
        eprintln!("sonkore: interface language {lang}: {e}");
    }
}

/// The language writes decimals with a comma.
pub fn decimal_comma() -> bool {
    LANG.get().is_some_and(|l| ["es", "de", "fr", "it", "pt"].contains(&l.split('_').next().unwrap_or("")))
}

/// A number typed in a field, or sent by Slint (which writes the language's
/// decimal comma): "3,5" and "3.5" alike. These fields never group thousands.
pub fn parse_number(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `value` with `decimals` digits as the language writes it.
pub fn locale_number(value: f64, decimals: usize) -> String {
    let s = format!("{value:.decimals$}");
    if decimal_comma() { s.replace('.', ",") } else { s }
}

/// The 'g' format at precision 6: up to six significant digits, no
/// trailing zeros (44.1, 96, 2.8224); `locale` writes the language's comma.
pub fn number_g(value: f64, locale: bool) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".into();
    }
    let digits = value.abs().log10().floor() as i32 + 1;
    let decimals = (6 - digits).max(0) as usize;
    let mut s = format!("{value:.decimals$}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    if locale && decimal_comma() { s.replace('.', ",") } else { s }
}

/// `text` of context `context`, translated (else as it is).
pub fn tr(context: &str, text: &str) -> String {
    TABLE.get().and_then(|t| t.get(&(context.to_owned(), text.to_owned()))).cloned().unwrap_or_else(|| text.to_owned())
}

/// tr() with {0}, {1}… replaced.
pub fn trf(context: &str, text: &str, args: &[&str]) -> String {
    let mut s = tr(context, text);
    for (i, a) in args.iter().enumerate().rev() {
        s = s.replace(&format!("{{{i}}}"), a);
    }
    s
}

/// A .po string: the quoted parts of its lines joined, escapes resolved.
fn po_string(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines {
        let quoted = line.trim().trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '_' || c == ' ');
        let Some(body) = quoted.strip_prefix('"').and_then(|q| q.strip_suffix('"')) else { continue };
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            }
        }
    }
    out
}

/// (msgctxt, msgid) -> msgstr, with Slint's doubled braces made single, as
/// the Rust texts write them. Untranslated entries are left out.
fn parse(po: &str) -> HashMap<(String, String), String> {
    let mut table = HashMap::new();
    for entry in po.split("\n\n") {
        let (mut ctx, mut id, mut text) = (Vec::new(), Vec::new(), Vec::new());
        let mut field = None;
        for line in entry.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with("msgctxt ") {
                field = Some(0);
            } else if line.starts_with("msgid ") {
                field = Some(1);
            } else if line.starts_with("msgstr ") {
                field = Some(2);
            }
            match field {
                Some(0) => ctx.push(line),
                Some(1) => id.push(line),
                Some(2) => text.push(line),
                _ => {}
            }
        }
        let single = |s: String| s.replace("{{", "{").replace("}}", "}");
        let (ctx, id, text) = (po_string(&ctx), single(po_string(&id)), single(po_string(&text)));
        if !id.is_empty() && !text.is_empty() {
            table.insert((ctx, id), text);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_a_comma_or_a_point() {
        assert_eq!(parse_number("-3,5"), Some(-3.5));
        assert_eq!(parse_number(" 2.25 "), Some(2.25));
        assert_eq!(parse_number("12"), Some(12.0));
        assert_eq!(parse_number("1,2,3"), None);
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn po_entries_with_escapes_and_continued_lines() {
        let po = "msgid \"\"\nmsgstr \"\"\n\"Content-Type: text/plain; charset=UTF-8\\n\"\n\n\
                  msgctxt \"Ctx\"\nmsgid \"Say \\\"{0}\\\"\\nnow {{x}}\"\nmsgstr \"Decí \\\"{0}\\\"\\n\"\n\"ya {{x}}\"\n\n\
                  msgctxt \"Ctx\"\nmsgid \"Untranslated\"\nmsgstr \"\"\n";
        let t = parse(po);
        assert_eq!(t.len(), 1);
        assert_eq!(t[&("Ctx".into(), "Say \"{0}\"\nnow {x}".into())], "Decí \"{0}\"\nya {x}");
    }

    #[test]
    fn every_language_file_reads() {
        for (lang, po) in FILES {
            let t = parse(po);
            assert!(t.len() > 800, "{lang}: {}", t.len());
            assert!(t.contains_key(&("TracksPage".into(), "Add music folder".into())), "{lang}");
        }
    }
}
