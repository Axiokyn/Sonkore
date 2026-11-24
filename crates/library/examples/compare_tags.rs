//! Reads every track of a library.json written by an earlier version again
//! with this crate's readers and prints the fields that differ ("ad", the
//! date added, aside):
//!   cargo run -p sonkore-library --example compare_tags -- <library.json>
use sonkore_library::{TrackMeta, metadata};

fn main() {
    let path = std::env::args().nth(1).expect("library.json");
    let root: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let (mut same, mut differ) = (0, 0);
    for entry in root["tracks"].as_array().unwrap() {
        let p = entry["p"].as_str().unwrap();
        let mut t = TrackMeta::default();
        metadata::read_into(p, &mut t);
        t.added = entry["ad"].as_i64().unwrap_or(0);
        let ours = t.to_json();
        let mut diffs = Vec::new();
        let keys: std::collections::BTreeSet<&String> =
            entry.as_object().unwrap().keys().chain(ours.as_object().unwrap().keys()).collect();
        for k in keys {
            let (a, b) = (entry.get(k.as_str()), ours.get(k.as_str()));
            let equal = match (a, b) {
                (Some(x), Some(y)) if x.is_number() && y.is_number() => x.as_f64() == y.as_f64(),
                _ => a == b,
            };
            if !equal {
                diffs.push(format!(
                    "{k}: stored {} / read now {}",
                    a.map(|v| v.to_string()).unwrap_or("-".into()),
                    b.map(|v| v.to_string()).unwrap_or("-".into())
                ));
            }
        }
        if diffs.is_empty() {
            same += 1;
        } else {
            differ += 1;
            println!("{}\n    {}", p, diffs.join("\n    "));
        }
    }
    println!("{same} identical, {differ} different");
}
