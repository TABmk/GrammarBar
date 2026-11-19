//! Local mistake statistics. One JSON line per checked part, kept in the app
//! folder and never sent anywhere. Only the changed fragments are stored, never
//! the checked text itself.
use crate::{align, edits::Segment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::Path,
};

const DAY: u64 = 86_400;
/// Longest stored fragment, in characters.
const FRAGMENT: usize = 60;
const REPEATED: usize = 8;

#[derive(Serialize, Deserialize)]
struct Record {
    /// Unix time in seconds.
    t: u64,
    /// Hashes of the checked part and of its corrected version. A part that was
    /// already counted, or that is GrammarBar's own output, is not counted again.
    h: String,
    out: String,
    words: usize,
    changes: Vec<Item>,
}
#[derive(Serialize, Deserialize, Clone)]
struct Item {
    /// Category, kind ("error" or "style"), original, replacement, explanation.
    c: String,
    k: String,
    o: String,
    r: String,
    e: String,
}

/// FNV-1a: stable across Rust versions, unlike the standard hasher.
fn hash(text: &str) -> String {
    let normalized = align::key(text.trim());
    let mut value: u64 = 0xcbf29ce484222325;
    for byte in normalized.bytes() {
        value ^= byte as u64;
        value = value.wrapping_mul(0x100000001b3);
    }
    format!("{value:016x}")
}
fn words(text: &str) -> usize {
    align::tokenize(text)
        .into_iter()
        .filter(|r| text[r.clone()].chars().any(char::is_alphanumeric))
        .count()
}
fn fragment(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(FRAGMENT) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.into(),
    }
}
fn load(path: &Path) -> Vec<Record> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Adds checked parts (source text and its segments) to the statistics.
pub fn record(path: &Path, parts: &[(&str, &[Segment])], now: u64) -> Result<(), String> {
    let existing = load(path);
    let mut seen: HashSet<String> = existing
        .iter()
        .flat_map(|r| [r.h.clone(), r.out.clone()])
        .collect();
    let mut lines = String::new();
    for &(source, segments) in parts {
        let h = hash(source);
        if source.trim().is_empty() || seen.contains(&h) {
            continue;
        }
        let corrected: String = segments.iter().map(|s| s.replacement.as_str()).collect();
        let record = Record {
            t: now,
            out: hash(&corrected),
            h,
            words: words(source),
            changes: segments
                .iter()
                .filter(|s| s.original != s.replacement)
                .map(|s| Item {
                    c: s.category.clone(),
                    k: s.kind.clone(),
                    o: fragment(&s.original),
                    r: fragment(&s.replacement),
                    e: s.explanation.trim().into(),
                })
                .collect(),
        };
        seen.insert(record.h.clone());
        seen.insert(record.out.clone());
        lines.push_str(&serde_json::to_string(&record).map_err(|e| e.to_string())?);
        lines.push('\n');
    }
    if lines.is_empty() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(lines.as_bytes()))
        .map_err(|_| "Could not save statistics.".into())
}

pub fn clear(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err("Could not clear statistics.".into())
        }
        _ => Ok(()),
    }
}

fn is_error(item: &Item) -> bool {
    item.k != "style"
}
/// Errors per 100 words, when anything was checked.
fn rate(errors: usize, words: usize) -> Option<f64> {
    (words > 0).then(|| errors as f64 * 100.0 / words as f64)
}
fn count_errors(records: &[&Record]) -> usize {
    records
        .iter()
        .map(|r| r.changes.iter().filter(|i| is_error(i)).count())
        .sum()
}
fn sum_words(records: &[&Record]) -> usize {
    records.iter().map(|r| r.words).sum()
}
fn by_category<'a>(records: &[&'a Record], errors: bool) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for item in records.iter().flat_map(|r| &r.changes) {
        if is_error(item) == errors {
            *counts.entry(item.c.as_str()).or_insert(0) += 1;
        }
    }
    counts
}
fn ranked(counts: HashMap<&str, usize>) -> Vec<(&str, usize)> {
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    list
}

/// Dashboard numbers for the last `days` days (`None`: all time).
pub fn summary(path: &Path, days: Option<u64>, now: u64) -> Value {
    let all = load(path);
    let start = days.map_or(0, |d| now.saturating_sub(d * DAY));
    let current: Vec<&Record> = all.iter().filter(|r| r.t >= start).collect();
    // The period before, for trends; none for "all time".
    let previous: Vec<&Record> = match days {
        Some(d) => {
            let before = start.saturating_sub(d * DAY);
            all.iter()
                .filter(|r| r.t >= before && r.t < start)
                .collect()
        }
        None => Vec::new(),
    };
    let (errors, words) = (count_errors(&current), sum_words(&current));
    let previous_words = sum_words(&previous);
    let previous_categories = by_category(&previous, true);
    let categories: Vec<Value> = ranked(by_category(&current, true))
        .into_iter()
        .map(|(id, count)| {
            json!({
                "id": id,
                "count": count,
                "rate": rate(count, words),
                "previousRate": rate(previous_categories.get(id).copied().unwrap_or(0), previous_words),
            })
        })
        .collect();
    let style: Vec<Value> = ranked(by_category(&current, false))
        .into_iter()
        .map(|(id, count)| json!({"id": id, "count": count}))
        .collect();

    // The same fix made again and again: the most useful list to learn from.
    let mut repeated: HashMap<(String, String), (usize, &Item)> = HashMap::new();
    for item in current.iter().flat_map(|r| &r.changes) {
        if !is_error(item) || item.o.is_empty() && item.r.is_empty() {
            continue;
        }
        let key = (item.o.to_lowercase(), item.r.to_lowercase());
        let entry = repeated.entry(key).or_insert((0, item));
        entry.0 += 1;
        entry.1 = item;
    }
    let mut repeated: Vec<_> = repeated.into_values().filter(|(n, _)| *n >= 2).collect();
    repeated.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.o.cmp(&b.1.o)));
    let repeated: Vec<Value> = repeated
        .into_iter()
        .take(REPEATED)
        .map(|(count, item)| json!({"original": item.o, "replacement": item.r, "category": item.c, "explanation": item.e, "count": count}))
        .collect();

    // The latest example of the most frequent mistake type.
    let example = categories.first().and_then(|top| {
        current
            .iter()
            .rev()
            .flat_map(|r| r.changes.iter().rev())
            .find(|i| is_error(i) && top["id"] == i.c.as_str())
            .map(|i| json!({"original": i.o, "replacement": i.r, "explanation": i.e}))
    });

    // Errors per 100 words over time: days for a week, weeks otherwise.
    let (bucket, buckets) = match days {
        Some(d) if d <= 7 => (DAY, d.max(1)),
        Some(d) => (7 * DAY, d.div_ceil(7)),
        None => (7 * DAY, 12),
    };
    let series: Vec<Value> = (0..buckets)
        .rev()
        .map(|i| {
            let end = now.saturating_sub(i * bucket);
            let from = end.saturating_sub(bucket);
            let slice: Vec<&Record> = all
                .iter()
                .filter(|r| r.t > from && r.t <= end || i == 0 && r.t > end)
                .collect();
            json!(rate(count_errors(&slice), sum_words(&slice)))
        })
        .collect();

    let clean = current
        .iter()
        .filter(|r| r.changes.iter().all(|i| !is_error(i)))
        .count();
    json!({
        "checks": current.len(),
        "words": words,
        "errors": errors,
        "styleCount": current.iter().flat_map(|r| &r.changes).filter(|i| !is_error(i)).count(),
        "rate": rate(errors, words),
        "previousRate": rate(count_errors(&previous), previous_words),
        "cleanShare": (!current.is_empty()).then(|| clean as f64 / current.len() as f64),
        "categories": categories,
        "style": style,
        "repeated": repeated,
        "example": example,
        "series": series,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(original: &str, replacement: &str, category: &str, kind: &str) -> Segment {
        Segment {
            original: original.into(),
            replacement: replacement.into(),
            explanation: if original == replacement {
                String::new()
            } else {
                "Why.".into()
            },
            category: category.into(),
            kind: kind.into(),
        }
    }
    fn file(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("grammarbar-{name}-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn counts_errors_and_style_separately_and_finds_repeats() {
        let path = file("counts");
        let now = 100 * DAY;
        let first = [
            segment("I want ", "I want ", "", ""),
            segment("informations", "information", "plurals", "error"),
            segment(" about it", " about it", "", ""),
        ];
        let second = [
            segment("Informations", "Information", "plurals", "error"),
            segment(" is ", " is ", "", ""),
            segment("very good", "excellent", "clarity", "style"),
        ];
        let clean = [segment("All good here.", "All good here.", "", "")];
        record(
            &path,
            &[("I want informations about it", &first)],
            now - 2 * DAY,
        )
        .unwrap();
        record(&path, &[("Informations is very good", &second)], now - DAY).unwrap();
        record(&path, &[("All good here.", &clean)], now).unwrap();
        let s = summary(&path, Some(30), now);
        assert_eq!(s["checks"], 3);
        assert_eq!(s["errors"], 2);
        assert_eq!(s["styleCount"], 1);
        assert_eq!(s["words"], 5 + 4 + 3);
        assert_eq!(s["categories"][0]["id"], "plurals");
        assert_eq!(s["style"][0]["id"], "clarity");
        assert_eq!(s["repeated"][0]["count"], 2);
        assert_eq!(s["repeated"][0]["replacement"], "Information");
        assert_eq!(s["example"]["original"], "Informations");
        let share = s["cleanShare"].as_f64().unwrap();
        assert!((share - 1.0 / 3.0).abs() < 1e-9, "{share}");
        assert_eq!(s["series"].as_array().unwrap().len(), 5);
        clear(&path).unwrap();
        assert_eq!(summary(&path, None, now)["checks"], 0);
        clear(&path).unwrap();
    }

    #[test]
    fn rechecks_and_own_output_are_not_counted_again() {
        let path = file("dedupe");
        let segments = [
            segment("He go", "He goes", "agreement", "error"),
            segment(" home.", " home.", "", ""),
        ];
        record(&path, &[("He go home.", &segments)], 10).unwrap();
        // Same text again (whitespace differs), then the corrected text itself.
        record(&path, &[("He go home.\n", &segments)], 20).unwrap();
        let output = [segment("He goes home.", "He goes home.", "", "")];
        record(&path, &[("He goes home.", &output)], 30).unwrap();
        let s = summary(&path, None, 40);
        assert_eq!(s["checks"], 1);
        assert_eq!(s["errors"], 1);
        clear(&path).unwrap();
    }

    #[test]
    fn trends_compare_with_the_previous_period() {
        let path = file("trend");
        let now = 30 * DAY;
        let bad = [segment("a apple", "an apple", "articles", "error")];
        let good = [segment("an orange", "an orange", "", "")];
        record(&path, &[("a apple", &bad)], now - 10 * DAY).unwrap();
        record(&path, &[("an orange", &good)], now - DAY).unwrap();
        let s = summary(&path, Some(7), now);
        assert_eq!(s["rate"], 0.0);
        assert_eq!(s["previousRate"], 50.0);
        assert_eq!(s["series"].as_array().unwrap().len(), 7);
        assert!(s["example"].is_null());
        let long = "x".repeat(200);
        assert_eq!(fragment(&long).chars().count(), FRAGMENT + 1);
        clear(&path).unwrap();
    }
}
