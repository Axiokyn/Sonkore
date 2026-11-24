//! The one way tracks are ordered by a field, shared by the
//! track lists and the play queue: the field, then what settles its ties,
//! numbers as numbers, text in natural order, paths folder by folder, and
//! blanks last whichever way the list is turned.

use std::cmp::Ordering;

use crate::collate::Collator;
use crate::meta::{FieldValue, TrackMeta};

/// The field, then its tie-breakers: an artist's tracks by album, disc and
/// track, an album's by disc and track…, and always the path last.
pub fn keys_for(field: &str) -> Vec<&str> {
    let mut keys = vec![field];
    if field == "genre" {
        keys.push("artist");
    }
    if matches!(field, "artist" | "albumArtist" | "genre" | "year") {
        keys.push("album");
    }
    if matches!(field, "artist" | "albumArtist" | "genre" | "year" | "album") {
        keys.push("discNo");
        keys.push("trackNo");
    }
    if field != "path" {
        keys.push("path");
    }
    keys
}

/// The non-empty parts of a path, between its slashes.
fn segments(p: &str) -> impl Iterator<Item = &str> {
    p.split('/').filter(|s| !s.is_empty())
}

/// Folder order: down the folders one by one, each folder's sub-folders
/// (with all they hold) before the files beside them.
pub fn compare_paths(collator: &Collator, a: &str, b: &str) -> Ordering {
    let x: Vec<&str> = segments(a).collect();
    let y: Vec<&str> = segments(b).collect();
    compare_segments(&x, &y, |s, t| collator.compare(s, t))
}

fn compare_segments<T>(x: &[T], y: &[T], cmp: impl Fn(&T, &T) -> Ordering) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        let x_file = i + 1 == x.len();
        let y_file = j + 1 == y.len();
        if x_file != y_file {
            return if x_file { Ordering::Greater } else { Ordering::Less };
        }
        let c = cmp(&x[i], &y[j]);
        if c != Ordering::Equal {
            return c;
        }
        i += 1;
        j += 1;
    }
    let (a_done, b_done) = (i >= x.len(), j >= y.len());
    if a_done == b_done {
        Ordering::Equal
    } else if a_done {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// A missing number is stored as 0 (no year, no track number).
fn zero_is_blank(key: &str) -> bool {
    matches!(key, "year" | "trackNo" | "discNo")
}

/// One key of two tracks. `None` is an invalid
/// value (a field the track has none of). Blanks go last either way, so
/// `descending` only says which way "last" is.
pub fn compare_key(
    collator: &Collator,
    key: &str,
    a: Option<&FieldValue>,
    b: Option<&FieldValue>,
    descending: bool,
) -> Ordering {
    if key == "path" {
        let s = |v: Option<&FieldValue>| v.map(FieldValue::to_text).unwrap_or_default();
        return compare_paths(collator, &s(a), &s(b));
    }
    let text = a.is_some_and(FieldValue::is_text) || b.is_some_and(FieldValue::is_text);
    let blank = |v: Option<&FieldValue>| -> bool {
        if text {
            v.map(FieldValue::to_text).unwrap_or_default().is_empty()
        } else {
            match v {
                None => true,
                Some(v) => zero_is_blank(key) && v.to_f64() == 0.0,
            }
        }
    };
    let (blank_a, blank_b) = (blank(a), blank(b));
    if blank_a != blank_b {
        return if blank_a != descending { Ordering::Greater } else { Ordering::Less };
    }
    if blank_a {
        return Ordering::Equal;
    }
    if text {
        let s = |v: Option<&FieldValue>| v.map(FieldValue::to_text).unwrap_or_default();
        return collator.compare(&s(a), &s(b));
    }
    let (x, y) = (a.map(FieldValue::to_f64).unwrap_or(0.0), b.map(FieldValue::to_f64).unwrap_or(0.0));
    x.partial_cmp(&y).unwrap_or(Ordering::Equal)
}

/// A value made ready for many comparisons (a sort): text as its
/// collation key, paths as their folders' keys.
#[derive(Clone, Debug)]
enum Prepared {
    Blank,
    Text(Vec<u32>),
    Number(f64),
    Path(Vec<Vec<u32>>),
}

/// Sorts `items` by `field` and its tie-breakers as the track lists do: a
/// stable sort, so equal rows keep their order; with `descending` only the
/// field itself turns round (its tie-breakers stay in order, blanks last).
/// `value(item, key)` gives an item's value for a key.
pub fn sort_by_field<T>(
    items: &mut Vec<T>,
    field: &str,
    descending: bool,
    value: impl Fn(&T, &str) -> Option<FieldValue>,
) {
    let collator = Collator::natural();
    let keys = keys_for(field);
    // Per key: is it text for any item (typing is per value, but a
    // role always has one type, so the first valid value decides)?
    let prepared: Vec<Vec<Prepared>> = items
        .iter()
        .map(|item| {
            keys.iter()
                .map(|&key| {
                    let v = value(item, key);
                    if key == "path" {
                        let p = v.map(|v| v.to_text()).unwrap_or_default();
                        return Prepared::Path(segments(&p).map(|s| collator.sort_key(s)).collect());
                    }
                    match v {
                        None => Prepared::Blank,
                        Some(FieldValue::Text(s)) if s.is_empty() => Prepared::Blank,
                        Some(FieldValue::Text(s)) => Prepared::Text(collator.sort_key(&s)),
                        Some(v) if zero_is_blank(key) && v.to_f64() == 0.0 => Prepared::Blank,
                        Some(v) => Prepared::Number(v.to_f64()),
                    }
                })
                .collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&l, &r| {
        for (k, _) in keys.iter().enumerate() {
            let down = k == 0 && descending;
            let c = match (&prepared[l][k], &prepared[r][k]) {
                (Prepared::Path(x), Prepared::Path(y)) => compare_segments(x, y, |s, t| s.cmp(t)),
                (Prepared::Blank, Prepared::Blank) => Ordering::Equal,
                (Prepared::Blank, _) => {
                    if down {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (_, Prepared::Blank) => {
                    if down {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                (Prepared::Text(x), Prepared::Text(y)) => x.cmp(y),
                (Prepared::Number(x), Prepared::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
                // Mixed text and numbers: the text rule (numbers as their text).
                (Prepared::Text(x), Prepared::Number(n)) => {
                    x.cmp(&collator.sort_key(&crate::json_out::format_double(*n)))
                }
                (Prepared::Number(n), Prepared::Text(y)) => {
                    collator.sort_key(&crate::json_out::format_double(*n)).cmp(y)
                }
                _ => Ordering::Equal,
            };
            if c != Ordering::Equal {
                return if down { c.reverse() } else { c };
            }
        }
        Ordering::Equal
    });
    apply_order(items, &order);
}

/// Reorders `items` so that position i holds the old `order[i]`.
pub(crate) fn apply_order<T>(items: &mut Vec<T>, order: &[usize]) {
    let mut old: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(order.iter().map(|&i| old[i].take().expect("a permutation")));
}

/// A track's value for a sort key, as the track lists give it (the
/// library's "favorite" role included).
pub fn track_value(t: &TrackMeta, key: &str, favorite: bool) -> Option<FieldValue> {
    if key == "favorite" {
        return Some(FieldValue::Bool(favorite));
    }
    t.value(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_before_files() {
        let c = Collator::natural();
        assert_eq!(compare_paths(&c, "/m/a/z.flac", "/m/b.flac"), Ordering::Less);
        assert_eq!(compare_paths(&c, "/m/b/x.flac", "/m/a.flac"), Ordering::Less);
        assert_eq!(compare_paths(&c, "/m/2.flac", "/m/10.flac"), Ordering::Less);
        assert_eq!(compare_paths(&c, "/m//a.flac", "/m/a.flac"), Ordering::Equal);
    }

    #[test]
    fn blanks_last_both_ways() {
        let c = Collator::natural();
        let y = |v: i64| Some(FieldValue::Int(v));
        assert_eq!(compare_key(&c, "year", y(0).as_ref(), y(1999).as_ref(), false), Ordering::Greater);
        assert_eq!(compare_key(&c, "year", y(0).as_ref(), y(1999).as_ref(), true), Ordering::Less);
        assert_eq!(keys_for("artist"), vec!["artist", "album", "discNo", "trackNo", "path"]);
        assert_eq!(keys_for("path"), vec!["path"]);
    }

    #[test]
    fn stable_field_sort() {
        let mut v = vec![("b", 2), ("a", 0), ("c", 1), ("d", 2)];
        sort_by_field(&mut v, "trackNo", false, |t, k| match k {
            "trackNo" => Some(FieldValue::Int(t.1)),
            "path" => Some(FieldValue::Text(format!("/{}", t.0))),
            _ => None,
        });
        assert_eq!(v.iter().map(|t| t.0).collect::<String>(), "cbda");
        sort_by_field(&mut v, "trackNo", true, |t, k| match k {
            "trackNo" => Some(FieldValue::Int(t.1)),
            "path" => Some(FieldValue::Text(format!("/{}", t.0))),
            _ => None,
        });
        assert_eq!(v.iter().map(|t| t.0).collect::<String>(), "bdca");
    }
}
