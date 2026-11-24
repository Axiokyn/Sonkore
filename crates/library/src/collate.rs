//! Text order as ICU's collation gives it (as earlier Sonkore versions
//! sorted), without ICU: a sort key per string whose plain comparison is the
//! collation.
//!
//! - Primary level: base letters case-folded, accents removed (each
//!   character decomposed, NFD, its combining marks set aside); blanks
//!   before punctuation, before symbols, before digits, before letters
//!   (ICU's root order of groups); in numeric mode a run of ASCII digits
//!   counts as its number ("Track 2" before "Track 10", leading zeros
//!   aside). A few letters expand as in ICU (ß = ss, æ = ae, œ = oe).
//! - Secondary level: the accents, character by character, a plain letter
//!   before an accented one.
//! - Tertiary level (case-sensitive collators only): lower case first.
//!
//! `natural()` is the track lists' collator (numeric, case-insensitive:
//! ICU's secondary strength); `locale()` is the locale-aware comparison
//! (tertiary, not numeric). Where ICU's tailoring for a language differs
//! from its root order (Swedish å after z, Spanish ñ as a letter of its
//! own…), this order follows the root.

use std::cmp::Ordering;

use unicode_normalization::char::{canonical_combining_class, decompose_canonical};

/// Groups, in ICU's root order.
const SPACE: u32 = 1;
const PUNCT: u32 = 2;
const SYMBOL: u32 = 3;
const DIGIT: u32 = 4;
const LETTER: u32 = 5;
const SHIFT: u32 = 22; // characters fit in 21 bits

#[derive(Clone, Copy, Debug)]
pub struct Collator {
    numeric: bool,
    case_sensitive: bool,
}

fn group(c: char) -> u32 {
    if c.is_whitespace() {
        SPACE
    } else if c.is_alphabetic() {
        LETTER
    } else if c.is_ascii_digit() {
        DIGIT
    } else if c.is_numeric() {
        LETTER // other number forms sort with the letters of their script
    } else if matches!(
        c,
        '_' | '-'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '.'
            | '\''
            | '"'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '@'
            | '*'
            | '/'
            | '\\'
            | '&'
            | '#'
            | '%'
    ) || matches!(
        c,
        '\u{2010}'..='\u{2027}' | '\u{00A1}' | '\u{00AB}' | '\u{00BB}' | '\u{00BF}' | '\u{3001}' | '\u{3002}'
    ) {
        PUNCT
    } else {
        SYMBOL
    }
}

/// One base character's primary weights (ß and the ligatures expand).
fn primary(c: char, out: &mut Vec<u32>) {
    let mut push = |c: char| out.push((LETTER << SHIFT) | c as u32);
    match c {
        'ß' | 'ẞ' => {
            push('s');
            push('s');
        }
        'æ' | 'Æ' => {
            push('a');
            push('e');
        }
        'œ' | 'Œ' => {
            push('o');
            push('e');
        }
        _ => {
            let g = group(c);
            let folded = if g == LETTER { c.to_lowercase().next().unwrap_or(c) } else { c };
            out.push((g << SHIFT) | folded as u32);
        }
    }
}

impl Collator {
    /// Natural order, case and accents aside.
    pub fn natural() -> Collator {
        Collator { numeric: true, case_sensitive: false }
    }

    /// Locale-aware: words as a dictionary orders them,
    /// lower case first, numbers digit by digit.
    pub fn locale() -> Collator {
        Collator { numeric: false, case_sensitive: true }
    }

    /// The sort key: comparing keys compares the strings.
    pub fn sort_key(&self, s: &str) -> Vec<u32> {
        let mut prim = Vec::with_capacity(s.len() + 2);
        let mut sec: Vec<u32> = Vec::new();
        let mut ter: Vec<u32> = Vec::new();
        let mut digits = String::new();
        let flush = |digits: &mut String, prim: &mut Vec<u32>, sec: &mut Vec<u32>, ter: &mut Vec<u32>| {
            if digits.is_empty() {
                return;
            }
            let trimmed = digits.trim_start_matches('0');
            let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
            prim.push((DIGIT << SHIFT) | trimmed.len() as u32);
            for d in trimmed.bytes() {
                prim.push((DIGIT << SHIFT) | d as u32);
            }
            sec.push(1);
            ter.push(0);
            digits.clear();
        };
        for c in s.chars() {
            let mut parts: Vec<char> = Vec::with_capacity(4);
            decompose_canonical(c, |d| parts.push(d));
            for d in parts {
                if canonical_combining_class(d) != 0 {
                    // An accent: on the secondary level, after its base.
                    flush(&mut digits, &mut prim, &mut sec, &mut ter);
                    sec.push(2 + d as u32);
                    continue;
                }
                if d.is_control() {
                    continue; // ignorable
                }
                if self.numeric && d.is_ascii_digit() {
                    digits.push(d);
                    continue;
                }
                flush(&mut digits, &mut prim, &mut sec, &mut ter);
                let before = prim.len();
                primary(d, &mut prim);
                for _ in before..prim.len() {
                    sec.push(1);
                    ter.push(u32::from(d.is_uppercase()));
                }
            }
        }
        flush(&mut digits, &mut prim, &mut sec, &mut ter);
        prim.push(0);
        prim.extend(sec);
        if self.case_sensitive {
            prim.push(0);
            prim.extend(ter);
        }
        prim
    }

    /// The collation order of `a` and `b`.
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        self.sort_key(a).cmp(&self.sort_key(b))
    }
}

/// Plain text order: UTF-16 code units.
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Case-insensitive order: code units after case folding.
pub fn case_insensitive_cmp(a: &str, b: &str) -> Ordering {
    let fold =
        |s: &str| -> Vec<u16> { s.chars().flat_map(|c| c.to_lowercase()).collect::<String>().encode_utf16().collect() };
    fold(a).cmp(&fold(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let c = Collator::natural();
        assert_eq!(c.compare("Track 2", "Track 10"), Ordering::Less);
        assert_eq!(c.compare("disc.cue#2", "disc.cue#10"), Ordering::Less);
        assert_eq!(c.compare("abc", "ABC"), Ordering::Equal);
        assert_eq!(c.compare("Éclair", "eclair"), Ordering::Greater); // the accent, after the letters
        assert_eq!(c.compare("Éclair", "ezra"), Ordering::Less);
        assert_eq!(c.compare("ñandú 2", "Ñandú 10"), Ordering::Less);
        assert_eq!(c.compare("01", "1"), Ordering::Equal);
        assert_eq!(c.compare("a b", "a-b"), Ordering::Less);
        assert_eq!(c.compare("9", "a"), Ordering::Less);
        assert_eq!(c.compare("Straße", "strasse"), Ordering::Equal);
    }

    #[test]
    fn locale_order() {
        let c = Collator::locale();
        assert_eq!(c.compare("a", "A"), Ordering::Less);
        assert_eq!(c.compare("A", "b"), Ordering::Less);
        assert_eq!(c.compare("10", "9"), Ordering::Less);
    }
}
