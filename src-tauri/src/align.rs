//! Turns a model draft (corrected text + listed changes) into display segments.
//!
//! The source and the corrected text are compared word by word. Differences that
//! are invisible or purely typographic (Unicode normalization, exotic spaces,
//! zero-width characters, curly vs straight quotes) keep the original text, so a
//! model that silently "cleans" those can no longer break or pollute a result.
use crate::edits::{Change, Draft, Segment};
use std::ops::Range;
use unicode_normalization::UnicodeNormalization;

const MAX_DIFF_CELLS: usize = 4_000_000;

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || is_mark(c)
}
fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '’')
}
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}'
    )
}

/// Byte ranges of words (with inner apostrophes), whitespace runs, and single other characters.
pub fn tokenize(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let end_of = |i: usize| chars.get(i).map_or(text.len(), |&(at, _)| at);
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        let mut j = i + 1;
        if is_word(c) {
            while j < chars.len() {
                let next = chars[j].1;
                if is_word(next) {
                    j += 1;
                } else if is_apostrophe(next)
                    && chars.get(j + 1).is_some_and(|&(_, after)| is_word(after))
                {
                    j += 2;
                } else {
                    break;
                }
            }
        } else if c.is_whitespace() {
            while j < chars.len() && chars[j].1.is_whitespace() {
                j += 1;
            }
        }
        tokens.push(start..end_of(j));
        i = j;
    }
    tokens
}

/// Comparison key: equal keys mean "no visible difference worth showing".
pub fn key(text: &str) -> String {
    text.nfc()
        .filter_map(|c| match c {
            '\r' => None,
            c if is_invisible(c) => None,
            '\n' => Some('\n'),
            c if c.is_whitespace() => Some(' '),
            '‘' | '’' | '‛' | '′' => Some('\''),
            '“' | '”' | '„' | '″' => Some('"'),
            c => Some(c),
        })
        .collect()
}

/// One aligned step: `a` is a byte range in the source, `b` in the corrected text.
#[derive(Debug, Clone)]
struct Op {
    equal: bool,
    a: Range<usize>,
    b: Range<usize>,
}

fn diff(source: &str, corrected: &str) -> Vec<Op> {
    let ta = tokenize(source);
    let tb = tokenize(corrected);
    let ka: Vec<String> = ta.iter().map(|r| key(&source[r.clone()])).collect();
    let kb: Vec<String> = tb.iter().map(|r| key(&corrected[r.clone()])).collect();
    let mut ops: Vec<Op> = Vec::new();
    let mut push = |equal: bool, a: Range<usize>, b: Range<usize>| {
        if a.is_empty() && b.is_empty() {
            return;
        }
        match ops.last_mut() {
            Some(last) if last.equal == equal => {
                last.a.end = a.end;
                last.b.end = b.end;
            }
            _ => ops.push(Op { equal, a, b }),
        }
    };
    let at_a = |i: usize| ta.get(i).map_or(source.len(), |r| r.start);
    let at_b = |j: usize| tb.get(j).map_or(corrected.len(), |r| r.start);
    let (mut start, mut end_a, mut end_b) = (0, ta.len(), tb.len());
    while start < end_a && start < end_b && ka[start] == kb[start] {
        start += 1;
    }
    while end_a > start && end_b > start && ka[end_a - 1] == kb[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    push(true, 0..at_a(start), 0..at_b(start));
    let (n, m) = (end_a - start, end_b - start);
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        push(false, at_a(start)..at_a(end_a), at_b(start)..at_b(end_b));
    } else {
        // Longest common subsequence, filled from the end so it can be walked forwards.
        let width = m + 1;
        let mut lcs = vec![0u32; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * width + j] = if ka[start + i] == kb[start + j] {
                    lcs[(i + 1) * width + j + 1] + 1
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            let (x, y) = (start + i, start + j);
            if i < n && j < m && ka[x] == kb[y] {
                push(true, at_a(x)..at_a(x + 1), at_b(y)..at_b(y + 1));
                i += 1;
                j += 1;
            } else if j < m && (i == n || lcs[i * width + j + 1] >= lcs[(i + 1) * width + j]) {
                push(false, at_a(x)..at_a(x), at_b(y)..at_b(y + 1));
                j += 1;
            } else {
                push(false, at_a(x)..at_a(x + 1), at_b(y)..at_b(y));
                i += 1;
            }
        }
    }
    push(
        true,
        at_a(end_a)..source.len(),
        at_b(end_b)..corrected.len(),
    );
    ops
}

/// Where each listed change sits in the source, when it can be found.
fn locate(source: &str, changes: &[Change]) -> Vec<(Option<Range<usize>>, usize)> {
    let mut cursor = 0;
    changes
        .iter()
        .enumerate()
        .map(|(index, change)| {
            if change.original.is_empty() {
                return (None, index);
            }
            let found = source[cursor..]
                .find(&change.original)
                .map(|at| at + cursor)
                .or_else(|| source.find(&change.original));
            let range = found.map(|at| at..at + change.original.len());
            if let Some(range) = &range {
                cursor = range.end;
            }
            (range, index)
        })
        .collect()
}

/// Explanation and category for an edit the model did not list.
fn fallback_explanation(original: &str, replacement: &str) -> (&'static str, &'static str) {
    let letters = |text: &str| {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };
    if original.trim().is_empty() && replacement.trim().is_empty() {
        ("Spacing.", "spacing")
    } else if letters(original) == letters(replacement) {
        ("Punctuation.", "punctuation")
    } else if letters(original).to_lowercase() == letters(replacement).to_lowercase() {
        ("Capitalization.", "capitalization")
    } else {
        ("Correction.", "other")
    }
}

fn explain(
    source: &str,
    corrected: &str,
    op: &Op,
    located: &[(Option<Range<usize>>, usize)],
    changes: &[Change],
) -> Explained {
    let (a, b) = (&op.a, &corrected[op.b.clone()]);
    let mut picked: Vec<usize> = located
        .iter()
        .filter_map(|(range, index)| {
            let range = range.as_ref()?;
            let touches = if a.is_empty() {
                range.start <= a.start && a.start <= range.end
            } else {
                range.start < a.end && range.end > a.start
            };
            touches.then_some(*index)
        })
        .collect();
    // Insertions (and fragments the model misquoted) cannot be placed by position:
    // match them by their replacement text instead.
    let wanted = b.trim();
    let by_text = |c: &Change| {
        let replacement = c.replacement.trim();
        !wanted.is_empty()
            && !replacement.is_empty()
            && (wanted.contains(replacement) || replacement.contains(wanted))
    };
    picked.extend(
        located
            .iter()
            .filter(|(range, index)| range.is_none() && by_text(&changes[*index]))
            .map(|(_, index)| *index),
    );
    if picked.is_empty() {
        picked.extend(changes.iter().position(by_text));
    }
    let mut parts: Vec<&str> = Vec::new();
    for &index in &picked {
        let text = changes[index].explanation.trim();
        if !text.is_empty() && !parts.contains(&text) {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        let (text, category) = fallback_explanation(&source[a.clone()], b);
        return Explained {
            text: text.into(),
            category,
            kind: "error",
        };
    }
    // A merged edit is an error if any part of it is; its type is the first listed.
    let first = &changes[picked[0]];
    Explained {
        text: parts.join(" "),
        category: crate::edits::category(&first.category),
        kind: if picked
            .iter()
            .any(|&i| crate::edits::kind(&changes[i].kind) == "error")
        {
            "error"
        } else {
            "style"
        },
    }
}
struct Explained {
    text: String,
    category: &'static str,
    kind: &'static str,
}

pub fn align(source: &str, input: &Draft) -> Result<Vec<Segment>, String> {
    if input.corrected.trim().is_empty() && !source.trim().is_empty() {
        return Err("The model returned an empty text.".into());
    }
    // Models often drop or add whitespace at the edges of a part (a final line
    // break, say). That is never a useful edit, so the source's edges are kept.
    let body = input.corrected.trim();
    let lead = &source[..source.len() - source.trim_start().len()];
    let trail = &source[source.trim_end().len()..];
    let corrected = if source.trim().is_empty() {
        source.to_owned()
    } else {
        format!("{lead}{body}{trail}")
    };
    let changes = &input.changes;
    let mut ops = diff(source, &corrected);
    // Invisible-only differences are not edits: keep the user's original characters.
    for op in &mut ops {
        if !op.equal && key(&source[op.a.clone()]) == key(&corrected[op.b.clone()]) {
            op.equal = true;
        }
    }
    let located = locate(source, changes);
    // When the model describes one change ("Me and Tom" → "Tom and I") that the
    // word diff splits into several, show it as one, unless that would swallow
    // more than a couple of unchanged words.
    let mut k = 1;
    while k + 1 < ops.len() {
        let (before, gap, after) = (&ops[k - 1], &ops[k], &ops[k + 1]);
        let gap_text = &source[gap.a.clone()];
        let words = tokenize(gap_text)
            .into_iter()
            .filter(|r| gap_text[r.clone()].chars().any(is_word))
            .count();
        let same_change = located.iter().any(|(range, _)| {
            range
                .as_ref()
                .is_some_and(|r| r.start <= before.a.start && after.a.end <= r.end)
        });
        if gap.equal
            && !before.equal
            && !after.equal
            && same_change
            && words <= 2
            && !gap_text.contains('\n')
        {
            ops[k - 1].a.end = ops[k + 1].a.end;
            ops[k - 1].b.end = ops[k + 1].b.end;
            ops.drain(k..k + 2);
        } else {
            k += 1;
        }
    }
    // A gap of spaces or one punctuation mark between two edits reads better as one
    // change ("has went" → "had gone"). Line breaks are never swallowed.
    let mut k = 1;
    while k + 1 < ops.len() {
        let gap = &source[ops[k].a.clone()];
        let small = !gap.contains('\n')
            && (gap.chars().all(char::is_whitespace)
                || gap.chars().count() == 1 && !gap.chars().all(is_word));
        if ops[k].equal && !ops[k - 1].equal && !ops[k + 1].equal && small {
            ops[k - 1].a.end = ops[k + 1].a.end;
            ops[k - 1].b.end = ops[k + 1].b.end;
            ops.drain(k..k + 2);
        } else {
            k += 1;
        }
    }
    let mut segments: Vec<Segment> = Vec::new();
    for op in &ops {
        let original = &source[op.a.clone()];
        let segment = if op.equal {
            Segment {
                original: original.into(),
                replacement: original.into(),
                ..Default::default()
            }
        } else {
            let explained = explain(source, &corrected, op, &located, changes);
            Segment {
                original: original.into(),
                replacement: corrected[op.b.clone()].into(),
                explanation: explained.text,
                category: explained.category.into(),
                kind: explained.kind.into(),
            }
        };
        match segments.last_mut() {
            Some(last) if segment.explanation.is_empty() && last.explanation.is_empty() => {
                last.original.push_str(&segment.original);
                last.replacement.push_str(&segment.replacement);
            }
            _ => segments.push(segment),
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(corrected: &str, changes: &[(&str, &str, &str)]) -> Draft {
        Draft {
            corrected: corrected.into(),
            changes: changes
                .iter()
                .map(|&(original, replacement, explanation)| Change {
                    original: original.into(),
                    replacement: replacement.into(),
                    explanation: explanation.into(),
                    category: "word_choice".into(),
                    kind: "error".into(),
                })
                .collect(),
        }
    }
    fn changed(segments: &[Segment]) -> Vec<(&str, &str, &str)> {
        segments
            .iter()
            .filter(|s| s.original != s.replacement)
            .map(|s| {
                (
                    s.original.as_str(),
                    s.replacement.as_str(),
                    s.explanation.as_str(),
                )
            })
            .collect()
    }
    fn check(source: &str, segments: &[Segment]) {
        assert_eq!(
            segments
                .iter()
                .map(|s| s.original.as_str())
                .collect::<String>(),
            source
        );
        for s in segments {
            assert_eq!(
                s.original == s.replacement,
                s.explanation.is_empty(),
                "{s:?}"
            );
        }
    }

    /// Whatever the model returns, the segments rebuild the source exactly and
    /// the replacements rebuild the corrected text.
    #[test]
    fn segments_round_trip_for_many_answers() {
        let cases = [
            ("I has a apple.", "I have an apple."),
            ("Me and Tom goes home", "Tom and I go home."),
            ("Hello,world !", "Hello, world!"),
            ("line one\n\nline two", "Line one.\n\nLine two."),
            ("café e\u{301} naïve", "café é naive"),
            ("🙂🙂 smile", "🙂 smile"),
            ("“quoted” text", "\"quoted\" text"),
            ("tab\tseparated", "tab separated"),
            ("crlf\r\nline", "CRLF\r\nline"),
            ("same text", "same text"),
            ("a b c d e f", "f e d c b a"),
            ("one", "completely different sentence here"),
        ];
        for (source, corrected) in cases {
            let segments = align(source, &draft(corrected, &[])).unwrap();
            check(source, &segments);
            let rebuilt: String = segments.iter().map(|s| s.replacement.as_str()).collect();
            // Characters the app treats as invisible keep their original form.
            assert_eq!(key(&rebuilt), key(corrected), "{source:?} -> {corrected:?}");
            for pair in segments.windows(2) {
                assert!(
                    pair[0].original != pair[0].replacement
                        || pair[1].original != pair[1].replacement,
                    "unchanged segments are merged: {pair:?}"
                );
            }
        }
    }

    #[test]
    fn tokens_cover_the_text() {
        let text = "I don't  know—café e\u{301}🙂\n\t“x”";
        let tokens = tokenize(text);
        assert_eq!(
            tokens.iter().map(|r| &text[r.clone()]).collect::<String>(),
            text
        );
        assert!(tokens.iter().any(|r| &text[r.clone()] == "don't"));
        assert!(tokens.iter().any(|r| &text[r.clone()] == "e\u{301}"));
    }

    #[test]
    fn edits_are_narrow_and_explained() {
        let source = "Yesterday I has went to a store and buyed apple.";
        let d = draft(
            "Yesterday I had gone to a store and bought an apple.",
            &[
                ("has went", "had gone", "Past perfect."),
                ("buyed", "bought", "Irregular verb."),
                ("", "an ", "Article."),
            ],
        );
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(
            changed(&segments),
            [
                ("has went", "had gone", "Past perfect."),
                ("buyed", "bought an", "Irregular verb. Article.")
            ]
        );
        assert_eq!(
            segments
                .iter()
                .map(|s| s.replacement.as_str())
                .collect::<String>(),
            "Yesterday I had gone to a store and bought an apple."
        );
    }

    #[test]
    fn invisible_normalization_keeps_the_original() {
        let source = "Hi\u{2009}Sarah,\u{200B} the cafe\u{301} on page\u{a0}4 have “quotes”.";
        let d = draft(
            "Hi Sarah, the café on page 4 has \"quotes\".",
            &[("have", "has", "Agreement.")],
        );
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(changed(&segments), [("have", "has", "Agreement.")]);
        let text: String = segments.iter().map(|s| s.replacement.as_str()).collect();
        assert_eq!(text, source.replace("have", "has"));
    }

    #[test]
    fn unlisted_edits_get_a_fallback_and_paragraphs_stay_apart() {
        let source = "One  two\n\nthree four";
        let d = draft("One two\n\nThree four", &[]);
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(
            changed(&segments),
            [
                ("  ", " ", "Spacing."),
                ("three", "Three", "Capitalization.")
            ]
        );
    }

    #[test]
    fn one_listed_change_is_not_split_by_the_diff() {
        let source = "Me and Tom will update them.";
        let d = draft(
            "Tom and I will update them.",
            &[("Me and Tom", "Tom and I", "Use the subject pronoun.")],
        );
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(
            changed(&segments),
            [("Me and Tom", "Tom and I", "Use the subject pronoun.")]
        );
        // A sentence-wide listed change still shows narrow edits.
        let source = "I has a cat and she like milk very much.";
        let d = draft(
            "I have a cat and she likes milk very much.",
            &[(
                source,
                "I have a cat and she likes milk very much.",
                "Agreement.",
            )],
        );
        let segments = align(source, &d).unwrap();
        assert_eq!(
            changed(&segments),
            [
                ("has", "have", "Agreement."),
                ("like", "likes", "Agreement.")
            ]
        );
    }

    #[test]
    fn insertions_use_the_listed_explanation() {
        let source = "Thanks you";
        let d = draft(
            "Thank you.",
            &[
                ("Thanks", "Thank", "Use thank."),
                ("", ".", "End with a period."),
            ],
        );
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(
            changed(&segments),
            [
                ("Thanks", "Thank", "Use thank."),
                ("", ".", "End with a period.")
            ]
        );
    }

    #[test]
    fn edge_whitespace_comes_from_the_source() {
        let source = "Hi\u{a0}team, me and him goes to the cafe\u{301} everyday.\n";
        let d = draft(
            "Hi team, he and I go to the café every day.",
            &[
                ("me and him", "he and I", "Subject pronouns."),
                ("goes", "go", "Plural verb."),
                ("everyday", "every day", "Two words."),
            ],
        );
        let segments = align(source, &d).unwrap();
        check(source, &segments);
        assert_eq!(
            changed(&segments),
            [
                (
                    "me and him goes",
                    "he and I go",
                    "Subject pronouns. Plural verb."
                ),
                ("everyday", "every day", "Two words.")
            ]
        );
        assert!(segments.last().unwrap().replacement.ends_with(".\n"));
    }

    #[test]
    fn unchanged_and_empty_answers() {
        let source = "All good.\n";
        assert_eq!(align(source, &draft(source, &[])).unwrap().len(), 1);
        assert!(align(source, &draft(" ", &[])).is_err());
    }
}
