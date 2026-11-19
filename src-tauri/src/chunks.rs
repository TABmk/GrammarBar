//! Splits long texts into parts that are corrected independently (and in parallel).
//! Concatenating the parts always gives back the exact source.

/// Characters per part: big enough for context, small enough to answer in ~20 s.
pub const TARGET: usize = 1800;

/// Pieces that end after a separator matched by `is_break` at byte `i`.
fn pieces(text: &str, is_break: impl Fn(&str, usize) -> Option<usize>) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < text.len() {
        if let Some(end) = is_break(text, i) {
            out.push(&text[start..end]);
            start = end;
            i = end;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Paragraphs: break after a run of whitespace that contains two line breaks.
fn paragraphs(text: &str) -> Vec<&str> {
    pieces(text, |t, i| {
        if !t[i..].starts_with('\n') {
            return None;
        }
        let run = t[i..]
            .find(|c: char| !c.is_whitespace())
            .map_or(t.len(), |n| i + n);
        (t[i..run].matches('\n').count() >= 2).then_some(run)
    })
}

/// Sentences: break after . ! ? … followed by whitespace (kept with the sentence).
fn sentences(text: &str) -> Vec<&str> {
    pieces(text, |t, i| {
        let c = t[i..].chars().next()?;
        if !matches!(c, '.' | '!' | '?' | '…') {
            return None;
        }
        let after = i + c.len_utf8();
        let run = t[after..]
            .find(|c: char| !c.is_whitespace())
            .map_or(t.len(), |n| after + n);
        (run > after).then_some(run)
    })
}

/// Last resort for one giant sentence: cut at the last space before the limit.
fn hard_split(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.chars().count() > TARGET {
        let limit = rest
            .char_indices()
            .nth(TARGET)
            .map_or(rest.len(), |(at, _)| at);
        let cut = rest[..limit]
            .char_indices()
            .rev()
            .find(|&(at, c)| at > 0 && c.is_whitespace())
            .map_or(limit, |(at, c)| at + c.len_utf8());
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

fn pack<'a>(units: Vec<&'a str>, text: &'a str, out: &mut Vec<&'a str>) {
    // Units are consecutive slices of `text`, so a part is a byte range of it.
    let base = text.as_ptr() as usize;
    let (mut start, mut end, mut size) = (None::<usize>, 0, 0);
    for unit in units {
        let at = unit.as_ptr() as usize - base;
        let len = unit.chars().count();
        if let Some(from) = start {
            if size + len > TARGET {
                out.push(&text[from..end]);
                start = None;
            }
        }
        if start.is_none() {
            start = Some(at);
            size = 0;
        }
        end = at + unit.len();
        size += len;
    }
    if let Some(from) = start {
        out.push(&text[from..end]);
    }
}

pub fn split(text: &str) -> Vec<&str> {
    let mut units = Vec::new();
    for paragraph in paragraphs(text) {
        if paragraph.chars().count() <= TARGET {
            units.push(paragraph);
            continue;
        }
        for sentence in sentences(paragraph) {
            if sentence.chars().count() <= TARGET {
                units.push(sentence);
            } else {
                units.extend(hard_split(sentence));
            }
        }
    }
    let mut out = Vec::new();
    pack(units, text, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pseudo-random texts of words, punctuation, line breaks and multi-byte
    /// characters always split losslessly into parts within the limit.
    #[test]
    fn random_texts_split_losslessly() {
        let words = [
            "word", "café", "🙂", "Hello.", "why?", "wow!", "\n", "\n\n", "  ", "…", "x",
        ];
        let mut seed: u64 = 42;
        for round in 0..200 {
            let mut text = String::new();
            let length = round * 37 % 2500;
            for _ in 0..length {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                text.push_str(words[(seed >> 33) as usize % words.len()]);
                if seed >> 40 & 1 == 0 {
                    text.push(' ');
                }
            }
            let parts = split(&text);
            assert_eq!(parts.concat(), text);
            assert!(parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().count() <= TARGET));
        }
    }

    #[test]
    fn short_text_is_one_part() {
        assert_eq!(split("Hello world.\n\nBye."), ["Hello world.\n\nBye."]);
        assert!(split("").is_empty());
    }

    #[test]
    fn long_text_splits_on_paragraphs_then_sentences() {
        let paragraph = "Sentence number one is here. ".repeat(40);
        let text = format!("{paragraph}\n\n{paragraph}\n\n🙂 {}", "word ".repeat(1000));
        let parts = split(&text);
        assert_eq!(parts.concat(), text);
        assert!(parts.len() > 3);
        assert!(parts.iter().all(|p| p.chars().count() <= TARGET));
        // Paragraph boundaries are respected when they fit.
        assert!(parts[0].ends_with("\n\n"));
        let no_spaces = "x".repeat(5000);
        let parts = split(&no_spaces);
        assert_eq!(parts.concat(), no_spaces);
        assert!(parts.iter().all(|p| p.chars().count() <= TARGET));
    }
}
