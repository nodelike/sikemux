//! The app's fuzzy ranking (src/lib/fuzzy.ts): a substring beats letters in order, an
//! earlier match beats a later one, and one at a word's start beats one inside a word.

use std::borrow::Cow;
use std::collections::BinaryHeap;

use memchr::memmem::Finder;

const SUBSEQUENCE: u32 = 1_000_000;

/// The text in lower case, folded into `buffer` when it fits so ranking a long list allocates nothing.
fn lower<'a>(text: &'a str, buffer: &'a mut [u8; 512]) -> Cow<'a, [u8]> {
    if !text.is_ascii() || text.len() > buffer.len() {
        return Cow::Owned(text.to_lowercase().into_bytes());
    }
    let folded = &mut buffer[..text.len()];
    folded.copy_from_slice(text.as_bytes());
    folded.make_ascii_lowercase();
    Cow::Borrowed(folded)
}

fn score_field(query: &Finder, text: &[u8]) -> Option<u32> {
    if let Some(at) = query.find(text) {
        let word_start = at == 0 || !text[at - 1].is_ascii_alphanumeric();
        return Some(if word_start { 0 } else { 1000 } + at as u32);
    }
    let (mut from, mut score, mut previous) = (0, 0, usize::MAX - 1);
    for &letter in query.needle() {
        let found = from + memchr::memchr(letter, &text[from..])?;
        score += if found == previous.wrapping_add(1) {
            0
        } else {
            found as u32
        };
        previous = found;
        from = found + 1;
    }
    Some(SUBSEQUENCE + score)
}

/// The places in `paths` of the best `limit` matches for `query`, best first, matched on the
/// file's name and then on its whole path. Once any path holds the query whole, paths that
/// only hold its letters in order are dropped.
pub fn rank_paths(query: &str, paths: &[String], limit: usize) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..paths.len().min(limit)).collect();
    }
    let query = Finder::new(query.as_bytes());
    let mut buffer = [0; 512];
    let mut best = BinaryHeap::with_capacity(limit + 1);
    let mut substring = false;
    for (place, path) in paths.iter().enumerate() {
        let path = lower(path, &mut buffer);
        let Some(whole) =
            score_field(&query, &path).filter(|whole| !substring || *whole < SUBSEQUENCE)
        else {
            continue;
        };
        let name = path
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(&path[..], |slash| &path[slash + 1..]);
        let score = score_field(&query, name).map_or(whole + 1, |name| name.min(whole + 1));
        if score < SUBSEQUENCE {
            if !substring {
                best.clear();
            }
            substring = true;
        } else if substring {
            continue;
        }
        best.push((score, place));
        if best.len() > limit {
            best.pop();
        }
    }
    best.into_sorted_vec()
        .into_iter()
        .map(|(_, place)| place)
        .collect()
}

/// The places in `texts` of the best `limit` matches for `query`, best first and earlier
/// places first among equals. Once any text holds the query whole, texts that only hold its
/// letters in order are dropped.
pub fn rank<'a>(query: &str, texts: impl IntoIterator<Item = &'a str>, limit: usize) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..texts.into_iter().take(limit).count()).collect();
    }
    let query = Finder::new(query.as_bytes());
    let mut buffer = [0; 512];
    let mut best = BinaryHeap::with_capacity(limit + 1);
    let mut substring = false;
    for (place, text) in texts.into_iter().enumerate() {
        let text = lower(text, &mut buffer);
        let Some(score) = score_field(&query, &text) else {
            continue;
        };
        if score < SUBSEQUENCE {
            if !substring {
                best.clear();
            }
            substring = true;
        } else if substring {
            continue;
        }
        best.push((score, place));
        if best.len() > limit {
            best.pop();
        }
    }
    best.into_sorted_vec()
        .into_iter()
        .map(|(_, place)| place)
        .collect()
}
