//! Equalizer APO / AutoEQ "ParametricEQ.txt" text: reading and writing.
//!
//! ```text
//! Preamp: -6.2 dB
//! Filter 1: ON PK Fc 105 Hz Gain 5.6 dB Q 0.70
//! Filter 2: ON LSC 10.8 dB Fc 105 Hz Gain 5.6 dB
//! ```

use std::sync::LazyLock;

use regex::Regex;

use crate::controller::Band;
use crate::numfmt::{number_f, number_g, parse_double};
use crate::super_eq::MAX_BANDS;

// The original patterns, with the ASCII meaning of \s and \d.
const S: &str = r"[\t\n\x0B\f\r ]";
static PREAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)^{S}*Preamp{S}*:{S}*([-+]?[0-9.]+){S}*dB")).unwrap());
static FILTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^{S}*Filter{S}*[0-9]*{S}*:{S}*(ON|OFF){S}+([A-Z]+)(?:{S}+([0-9.]+){S}*dB)?{S}+Fc{S}+([0-9.]+){S}*Hz(?:{S}+Gain{S}+([-+]?[0-9.]+){S}*dB)?(?:{S}+Q{S}+([0-9.]+)|{S}+BW{S}+Oct{S}+([0-9.]+))?"
    ))
    .unwrap()
});

/// The preamp and bands of a parametric EQ text, or None when it has no
/// filter line. At most [`MAX_BANDS`] bands are kept.
///
/// * Type aliases: PEQ → PK; LS / LSQ → LSC; HS / HSQ → HSC; LP → LPQ;
///   HP → HPQ.
/// * A missing gain is 0 dB; a missing Q is 0.7071. A bandwidth in octaves
///   ("BW Oct N") is the Q sqrt(2^N) / (2^N - 1).
/// * Numbers are taken as written (the controller clamps them), but never
///   infinite: digits past f64's range read as 0.
/// * Shelves given by slope instead of Q ("LSC 10.8 dB", or a plain
///   LS / HS, whose slope is S = 0.9): the RBJ slope S (x dB per octave is
///   S = x / 12, clamped to 0.05..1) as Q = 1 / sqrt((A + 1/A)(1/S - 1) + 2).
pub fn parse(text: &str) -> Option<(f64, Vec<Band>)> {
    let mut preamp = 0.0;
    let mut bands = Vec::new();
    for line in text.split('\n') {
        if let Some(m) = PREAMP.captures(line) {
            preamp = parse_double(&m[1]);
            continue;
        }
        let Some(m) = FILTER.captures(line) else {
            continue;
        };
        let group = |i: usize| m.get(i).map_or("", |g| g.as_str());
        let given = group(2).to_uppercase();
        let kind = match given.as_str() {
            "PEQ" => "PK",
            "LS" | "LSQ" => "LSC",
            "HS" | "HSQ" => "HSC",
            "LP" => "LPQ",
            "HP" => "HPQ",
            t => t,
        }
        .to_owned();
        let mut b = Band {
            enabled: group(1).eq_ignore_ascii_case("ON"),
            freq: parse_double(group(4)),
            gain: if group(5).is_empty() { 0.0 } else { parse_double(group(5)) },
            q: if !group(6).is_empty() {
                parse_double(group(6))
            } else if !group(7).is_empty() {
                // "BW Oct 0" (or less) has no Q: the default.
                let width = 2f64.powf(parse_double(group(7)));
                let q = width.sqrt() / (width - 1.0);
                if q.is_finite() && q > 0.0 { q } else { 0.7071 }
            } else {
                0.7071
            },
            kind,
        };
        if (b.kind == "LSC" || b.kind == "HSC") && group(6).is_empty() && group(7).is_empty() {
            let slope = if !group(3).is_empty() {
                crate::biquad::clamp(parse_double(group(3)) / 12.0, 0.05, 1.0)
            } else if given == "LS" || given == "HS" {
                0.9
            } else {
                0.0
            };
            if slope > 0.0 {
                let a = crate::biquad::pow10(b.gain.abs() / 40.0);
                b.q = 1.0 / ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt();
            }
        }
        if bands.len() < MAX_BANDS {
            bands.push(b);
        }
    }
    (!bands.is_empty()).then_some((preamp, bands))
}

/// The text an export writes:
/// `Preamp: %.1f dB` then `Filter n: ON|OFF TYPE Fc %g Hz Gain %.1f dB Q %.2f`.
pub fn export(preamp: f64, bands: &[Band]) -> String {
    let mut out = format!("Preamp: {} dB\n", number_f(preamp, 1));
    for (i, b) in bands.iter().enumerate() {
        out.push_str(&format!(
            "Filter {}: {}{} Fc {} Hz Gain {} dB Q {}\n",
            i + 1,
            if b.enabled { "ON " } else { "OFF " },
            b.kind,
            number_g(b.freq, 6),
            number_f(b.gain, 1),
            number_f(b.q, 2)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_autoeq_text() {
        let text = "Preamp: -6.2 dB\r\nFilter 1: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.70\r\n\
                    Filter 2: ON PK Fc 2363 Hz Gain -2.4 dB Q 1.43\nFilter 3: OFF HSC Fc 10000 Hz Gain 2.3 dB Q 0.70\n\
                    # comment\nFilter: ON PEQ Fc 50 Hz Gain +3 dB\nFilter 5: ON LS 12dB Fc 80 Hz Gain -4 dB\n\
                    Filter 6: ON HS Fc 9000 Hz Gain 6 dB\nFilter 7: ON NO Fc 60 Hz\nfilter 8: on lp fc 18000 hz q 0.5\n";
        let (preamp, bands) = parse(text).unwrap();
        assert_eq!(preamp, -6.2);
        assert_eq!(bands.len(), 8);
        assert_eq!((bands[0].kind.as_str(), bands[0].freq, bands[0].gain, bands[0].q), ("LSC", 105.0, 5.6, 0.70));
        assert!(!bands[2].enabled);
        assert_eq!((bands[3].kind.as_str(), bands[3].gain, bands[3].q), ("PK", 3.0, 0.7071));
        // LS 12 dB: slope 1 → Q = 1/sqrt(2).
        assert_eq!(bands[4].kind, "LSC");
        assert!((bands[4].q - 1.0 / 2f64.sqrt()).abs() < 1e-15);
        // Bare HS: S = 0.9.
        let a = 10f64.powf(6.0 / 40.0);
        assert!((bands[5].q - 1.0 / ((a + 1.0 / a) * (1.0 / 0.9 - 1.0) + 2.0).sqrt()).abs() < 1e-15);
        assert_eq!((bands[6].kind.as_str(), bands[6].gain), ("NO", 0.0));
        assert_eq!((bands[7].kind.as_str(), bands[7].freq, bands[7].q), ("LPQ", 18000.0, 0.5));
        assert!(parse("Preamp: 3 dB\nnothing").is_none());
    }

    #[test]
    fn bandwidth_in_octaves() {
        // Q = sqrt(2^N) / (2^N - 1): one octave is Q 1.4142, two 0.6667.
        let (_, bands) =
            parse("Filter 1: ON PK Fc 1000 Hz Gain 3 dB BW Oct 1\nFilter 2: ON PK Fc 200 Hz Gain -2 dB BW Oct 2.0\n")
                .unwrap();
        assert!((bands[0].q - 2f64.sqrt()).abs() < 1e-12, "{}", bands[0].q);
        assert!((bands[1].q - 2.0 / 3.0).abs() < 1e-12, "{}", bands[1].q);
        assert_eq!((bands[1].freq, bands[1].gain), (200.0, -2.0));
    }

    #[test]
    fn numbers_too_large_are_not_infinite() {
        let huge = "9".repeat(309);
        let (preamp, bands) =
            parse(&format!("Preamp: {huge} dB\nFilter 1: ON PK Fc {huge} Hz Gain 3 dB Q {huge}\n")).unwrap();
        assert!(preamp.is_finite() && bands[0].freq.is_finite() && bands[0].q.is_finite());
    }

    #[test]
    fn export_then_import() {
        let bands = vec![
            Band { kind: "PK".into(), freq: 31.25, gain: 3.25, q: 1.41421356237, enabled: true },
            Band { kind: "HSC".into(), freq: 12345.678, gain: -2.0, q: 0.7, enabled: false },
            Band { kind: "NO".into(), freq: 60.0, gain: 0.0, q: 10.0, enabled: true },
        ];
        let text = export(-4.25, &bands);
        assert_eq!(
            text,
            "Preamp: -4.3 dB\nFilter 1: ON PK Fc 31.25 Hz Gain 3.3 dB Q 1.41\n\
             Filter 2: OFF HSC Fc 12345.7 Hz Gain -2.0 dB Q 0.70\nFilter 3: ON NO Fc 60 Hz Gain 0.0 dB Q 10.00\n"
        );
        let (preamp, back) = parse(&text).unwrap();
        assert_eq!(preamp, -4.3);
        assert_eq!(back.len(), 3);
        assert_eq!((back[0].freq, back[0].gain, back[0].q), (31.25, 3.3, 1.41));
        assert_eq!((back[1].enabled, back[1].freq, back[1].q), (false, 12345.7, 0.7));
        assert_eq!(export(preamp, &back), text);
    }
}
