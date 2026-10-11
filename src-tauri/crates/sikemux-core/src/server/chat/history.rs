//! Everything a chat said, on disk, one event a line. The replay in memory
//! keeps only the newest part of a long chat; a phone that scrolls back past
//! it reads the rest from here, a page of whole turns at a time.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::protocol::{ChatEvent, ChatEventKind};

/// Past this the oldest half of the file goes, so a chat that never ends
/// cannot fill the disk. Its earliest turns are then out of reach.
pub(crate) const MAX_HISTORY_BYTES: u64 = 256 * 1024 * 1024;

/// An event a page can start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    index: u64,
    offset: u64,
    starts_turn: bool,
}

pub(crate) struct History {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
    max_bytes: u64,
    /// The index of the first event still in the file.
    first: u64,
    /// One past the index of the last event written.
    next: u64,
    points: Vec<Point>,
}

/// Events from before some index, and where the page before them starts.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Page {
    pub events: Vec<ChatEvent>,
    pub older_before: Option<u64>,
}

/// Removes what an earlier core left, which no chat of this one reads.
pub(crate) fn clear(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = fs::remove_file(entry.path());
    }
}

impl History {
    pub(crate) fn open(dir: &Path, name: &str, max_bytes: u64) -> Self {
        let path = dir.join(format!("{name}.jsonl"));
        let file = fs::create_dir_all(dir)
            .and_then(|()| {
                OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(&path)
            })
            .inspect_err(|error| {
                eprintln!(
                    "sikemux core: a chat's history is not kept on disk at {}: {error}",
                    path.display()
                )
            })
            .ok();
        Self {
            path,
            file,
            bytes: 0,
            max_bytes,
            first: 0,
            next: 0,
            points: Vec::new(),
        }
    }

    /// The index of the oldest event a page can still return.
    pub(crate) fn first(&self) -> Option<u64> {
        self.file.as_ref().map(|_| self.first)
    }

    pub(crate) fn append(&mut self, index: u64, event: &ChatEvent, point: bool, starts_turn: bool) {
        if self.file.is_none() {
            return;
        }
        if self.bytes == 0 {
            self.first = index;
        }
        let Ok(mut line) = serde_json::to_vec(event) else {
            return;
        };
        line.push(b'\n');
        let offset = self.bytes;
        let written = self
            .file
            .as_mut()
            .map_or(Ok(()), |file| file.write_all(&line));
        if let Err(error) = written {
            self.fail(&error);
            return;
        }
        self.bytes += line.len() as u64;
        self.next = index + 1;
        if point || offset == 0 {
            self.points.push(Point {
                index,
                offset,
                starts_turn: starts_turn || offset == 0,
            });
        }
        if self.bytes > self.max_bytes {
            if let Err(error) = self.drop_oldest_half() {
                self.fail(&error);
            }
        }
    }

    fn fail(&mut self, error: &io::Error) {
        eprintln!(
            "sikemux core: a chat's history stopped being kept on disk at {}: {error}",
            self.path.display()
        );
        self.file = None;
        self.points.clear();
        let _ = fs::remove_file(&self.path);
    }

    fn drop_oldest_half(&mut self) -> io::Result<()> {
        let half = self.bytes / 2;
        let after_half = |point: &&Point| point.offset >= half && point.offset > 0;
        let cut = self
            .points
            .iter()
            .find(|point| point.starts_turn && after_half(point))
            .or_else(|| self.points.iter().find(after_half))
            .copied();
        let Some(cut) = cut else {
            if let Some(file) = self.file.as_mut() {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
            }
            self.bytes = 0;
            self.first = self.next;
            self.points.clear();
            return Ok(());
        };
        let kept = self.path.with_extension("jsonl.kept");
        {
            let mut from = File::open(&self.path)?;
            from.seek(SeekFrom::Start(cut.offset))?;
            let mut to = File::create(&kept)?;
            io::copy(&mut from, &mut to)?;
        }
        fs::rename(&kept, &self.path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        self.file = Some(file);
        self.bytes -= cut.offset;
        self.first = cut.index;
        self.points.retain(|point| point.index >= cut.index);
        for point in &mut self.points {
            point.offset -= cut.offset;
        }
        if let Some(first) = self.points.first_mut() {
            first.starts_turn = true;
        }
        Ok(())
    }

    /// Forgets the event at `index` and everything after it. One that does
    /// not start a page cannot be found in the file, so everything goes.
    pub(crate) fn truncate(&mut self, index: u64) {
        if self.file.is_none() || index >= self.next {
            return;
        }
        let offset = match self.points.iter().find(|point| point.index == index) {
            Some(point) if index > self.first => point.offset,
            _ => 0,
        };
        let cut = self.file.as_mut().map_or(Ok(()), |file| {
            file.set_len(offset)
                .and_then(|()| file.seek(SeekFrom::Start(offset)).map(drop))
        });
        if let Err(error) = cut {
            self.fail(&error);
            return;
        }
        self.bytes = offset;
        self.points.retain(|point| point.offset < offset);
        if offset == 0 {
            self.first = index;
        }
        self.next = index;
    }

    /// Whole turns from before the event at `before`, as many as `turns`
    /// while they stay under `max_bytes`. A turn too big for a page on its
    /// own comes in pieces. Permission requests are left out: they were
    /// answered long ago.
    pub(crate) fn page(&self, before: u64, turns: usize, max_bytes: u64) -> io::Result<Page> {
        if self.file.is_none() || before <= self.first {
            return Ok(Page::default());
        }
        let end = match self
            .points
            .binary_search_by_key(&before, |point| point.index)
        {
            Ok(found) => self.points[found].offset,
            Err(_) if before >= self.next => self.bytes,
            Err(_) => return Err(io::Error::other("that point of the chat is not kept")),
        };
        let candidates = &self.points[..self.points.partition_point(|point| point.index < before)];
        let mut chosen = None;
        let mut fits = None;
        let mut counted = 0;
        for (position, point) in candidates.iter().enumerate().rev() {
            if end - point.offset > max_bytes {
                chosen = chosen.or(fits).or(Some(position));
                break;
            }
            fits = Some(position);
            if point.starts_turn {
                chosen = Some(position);
                counted += 1;
                if counted >= turns {
                    break;
                }
            }
        }
        let Some(chosen) = chosen.or(fits) else {
            return Ok(Page::default());
        };
        let start = candidates[chosen];
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(start.offset))?;
        let mut text = Vec::with_capacity((end - start.offset) as usize);
        file.take(end - start.offset).read_to_end(&mut text)?;
        let events = text
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_slice::<ChatEvent>(line).ok())
            .filter(|event| event.kind != ChatEventKind::PermissionRequest)
            .collect();
        Ok(Page {
            events,
            older_before: (chosen > 0).then_some(start.index),
        })
    }
}

impl Drop for History {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(kind: ChatEventKind, n: u64) -> ChatEvent {
        ChatEvent {
            kind,
            payload: json!({ "n": n }),
        }
    }

    fn numbers(page: &Page) -> Vec<u64> {
        page.events
            .iter()
            .map(|event| event.payload["n"].as_u64().unwrap_or(u64::MAX))
            .collect()
    }

    /// Three events a turn: the turn starting, a reply, and the turn ending.
    fn turns(history: &mut History, count: u64) {
        for turn in 0..count {
            let base = turn * 3;
            history.append(base, &event(ChatEventKind::TurnStarted, base), true, true);
            history.append(
                base + 1,
                &event(ChatEventKind::Error, base + 1),
                false,
                false,
            );
            history.append(
                base + 2,
                &event(ChatEventKind::TurnCompleted, base + 2),
                false,
                false,
            );
        }
    }

    #[test]
    fn pages_are_whole_turns_back_to_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 5);

        let page = history.page(9, 2, u64::MAX).unwrap();
        assert_eq!(numbers(&page), [3, 4, 5, 6, 7, 8]);
        assert_eq!(page.older_before, Some(3));

        let page = history.page(3, 2, u64::MAX).unwrap();
        assert_eq!(numbers(&page), [0, 1, 2]);
        assert_eq!(page.older_before, None);

        assert_eq!(history.page(0, 2, u64::MAX).unwrap(), Page::default());
        assert_eq!(
            numbers(&history.page(15, 1, u64::MAX).unwrap()),
            [12, 13, 14]
        );
    }

    #[test]
    fn a_rewind_forgets_from_its_turn_and_keeps_writing_after() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 4);
        history.truncate(6);
        assert_eq!(
            numbers(&history.page(6, 10, u64::MAX).unwrap()),
            [0, 1, 2, 3, 4, 5]
        );
        history.append(20, &event(ChatEventKind::TurnStarted, 20), true, true);
        assert_eq!(numbers(&history.page(21, 1, u64::MAX).unwrap()), [20]);
        assert_eq!(
            numbers(&history.page(20, 10, u64::MAX).unwrap()),
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn a_rewind_to_an_event_that_starts_no_page_forgets_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 3);
        history.truncate(4);
        assert_eq!(history.page(9, 10, u64::MAX).unwrap(), Page::default());
    }

    #[test]
    fn a_page_stops_at_its_size_but_holds_at_least_one_turn() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 4);
        let last_turn = history.bytes - history.points[3].offset;
        let page = history.page(12, 10, last_turn).unwrap();
        assert_eq!(numbers(&page), [9, 10, 11]);
        assert_eq!(page.older_before, Some(9));
        let page = history.page(12, 10, 1).unwrap();
        assert_eq!(numbers(&page), [9, 10, 11]);
    }

    #[test]
    fn answered_permission_requests_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        history.append(0, &event(ChatEventKind::TurnStarted, 0), true, true);
        history.append(1, &event(ChatEventKind::PermissionRequest, 1), false, false);
        history.append(2, &event(ChatEventKind::TurnCompleted, 2), false, false);
        assert_eq!(numbers(&history.page(3, 1, u64::MAX).unwrap()), [0, 2]);
    }

    #[test]
    fn past_its_size_the_oldest_half_goes() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 10);
        let size = history.bytes;
        let mut history = History::open(dir.path(), "capped", size - 1);
        turns(&mut history, 10);
        assert!(history.bytes < size);
        let first = history.first().unwrap();
        assert!(first > 0 && first.is_multiple_of(3));
        let page = history.page(30, 100, u64::MAX).unwrap();
        assert_eq!(numbers(&page), (first..30).collect::<Vec<_>>());
        assert_eq!(page.older_before, None);
        assert_eq!(history.page(first, 1, u64::MAX).unwrap(), Page::default());
    }

    #[test]
    fn the_file_goes_with_the_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 1);
        let path = history.path.clone();
        assert!(path.exists());
        drop(history);
        assert!(!path.exists());
    }
}
