//! The app's fuzzy ranking: a substring beats letters in order, an
//! earlier match beats a later one, and one at a word's start beats one inside a word.

use std::borrow::Cow;
use std::collections::BinaryHeap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use memchr::memmem::Finder;
use serde::Serialize;

const SUBSEQUENCE: u32 = 1_000_000;
/// How many paths are ranked between looks at whether the ranking is still wanted.
const STOP_EVERY: usize = 1024;

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
    rank_paths_until(query, paths, limit, &AtomicBool::new(false)).unwrap_or_default()
}

/// As `rank_paths`, given up with none as soon as `stopped` is set.
pub fn rank_paths_until(
    query: &str,
    paths: &[String],
    limit: usize,
    stopped: &AtomicBool,
) -> Option<Vec<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some((0..paths.len().min(limit)).collect());
    }
    let query = Finder::new(query.as_bytes());
    let mut buffer = [0; 512];
    let mut best = BinaryHeap::with_capacity(limit + 1);
    let mut substring = false;
    for (place, path) in paths.iter().enumerate() {
        if place % STOP_EVERY == 0 && stopped.load(Ordering::Relaxed) {
            return None;
        }
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
    Some(
        best.into_sorted_vec()
            .into_iter()
            .map(|(_, place)| place)
            .collect(),
    )
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

/// Where `query` falls in `text` as `rank` finds it, as byte ranges of `text`: the first
/// place it appears whole, else its letters in order, else nowhere.
pub fn matched(query: &str, text: &str) -> Vec<Range<usize>> {
    let query: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let letters: Vec<(usize, char)> = text
        .char_indices()
        .map(|(at, letter)| (at, letter.to_lowercase().next().unwrap_or(letter)))
        .collect();
    let end = |place: usize| letters.get(place + 1).map_or(text.len(), |(at, _)| *at);
    if let Some(start) = letters.windows(query.len()).position(|run| {
        run.iter()
            .zip(&query)
            .all(|((_, letter), wanted)| letter == wanted)
    }) {
        return std::iter::once(letters[start].0..end(start + query.len() - 1)).collect();
    }
    let mut wanted = query.iter().peekable();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (place, (at, letter)) in letters.iter().enumerate() {
        let Some(next) = wanted.peek() else {
            break;
        };
        if letter != *next {
            continue;
        }
        wanted.next();
        match ranges.last_mut() {
            Some(last) if last.end == *at => last.end = end(place),
            _ => ranges.push(*at..end(place)),
        }
    }
    match wanted.peek() {
        Some(_) => Vec::new(),
        None => ranges,
    }
}

/// A project's file or folder, as `rank_entries` finds it.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub folder: bool,
}

/// The best `limit` of `files` and the folders that hold them for `query`, a file before a
/// folder of the same name.
pub fn rank_entries(query: &str, files: &[String], limit: usize) -> Vec<Entry> {
    let mut folders: Vec<&str> = files
        .iter()
        .flat_map(|file| file.match_indices('/').map(|(at, _)| &file[..at]))
        .collect();
    folders.sort_unstable();
    folders.dedup();
    let entries: Vec<String> = files
        .iter()
        .cloned()
        .chain(folders.iter().map(|folder| (*folder).to_owned()))
        .collect();
    rank_paths(query, &entries, limit)
        .into_iter()
        .map(|place| Entry {
            path: entries[place].clone(),
            folder: place >= files.len(),
        })
        .collect()
}
