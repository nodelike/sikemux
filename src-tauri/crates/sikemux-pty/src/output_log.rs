use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

mod terminal_text;

pub const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Default)]
pub struct OutputLog {
    bytes: VecDeque<u8>,
    end: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OutputQuery {
    pub cursor: u64,
    pub limit: usize,
    pub tail: Option<usize>,
    pub search: Option<String>,
    pub context: usize,
    pub plain: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub bytes: Vec<u8>,
    pub cursor: u64,
    pub end: u64,
    pub truncated: bool,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<usize>,
}

const PLAIN_WINDOW: u64 = 256 * 1024;

impl OutputLog {
    pub fn push(&mut self, bytes: &[u8]) {
        self.end += bytes.len() as u64;
        if bytes.len() >= MAX_OUTPUT {
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - MAX_OUTPUT..]);
        } else {
            let overflow = (self.bytes.len() + bytes.len()).saturating_sub(MAX_OUTPUT);
            self.bytes.drain(..overflow);
            self.bytes.extend(bytes);
        }
    }

    /// The bytes still held, and the offset just past the last of them.
    pub fn contents(&self) -> (Vec<u8>, u64) {
        (self.bytes.iter().copied().collect(), self.end)
    }

    /// A log holding `bytes` as the newest output, ending at `end`.
    pub fn restored(bytes: &[u8], end: u64) -> Self {
        let kept = bytes
            .get(bytes.len().saturating_sub(MAX_OUTPUT)..)
            .unwrap_or_default();
        Self {
            bytes: kept.iter().copied().collect(),
            end: end.max(kept.len() as u64),
        }
    }

    fn start(&self) -> u64 {
        self.end - self.bytes.len() as u64
    }

    fn slice(&self, from: u64, to: u64) -> Vec<u8> {
        let start = self.start();
        self.bytes
            .range((from - start) as usize..(to - start) as usize)
            .copied()
            .collect()
    }

    pub fn query(&self, query: &OutputQuery) -> Result<OutputPage, String> {
        if query.cursor > self.end {
            return Err(format!(
                "output cursor {} is ahead of this execution (end {})",
                query.cursor, self.end
            ));
        }
        if query.limit == 0 || query.limit > 8192 {
            return Err("output limit must be between 1 and 8192 bytes".into());
        }
        if query.tail.is_some_and(|lines| lines == 0 || lines > 10_000) {
            return Err("tail must be between 1 and 10000 lines".into());
        }
        if query.context > 20 {
            return Err("context must be at most 20 lines".into());
        }
        match (&query.search, query.tail, query.plain) {
            (Some(needle), _, _) => self.search(needle, query),
            (None, Some(lines), false) => Ok(self.raw_tail(lines, query.limit)),
            (None, Some(lines), true) => Ok(self.plain_tail(lines, query.limit)),
            (None, None, true) => Ok(self.plain_page(query.cursor, query.limit)),
            (None, None, false) => self.read(query.cursor, query.limit),
        }
    }

    pub fn read(&self, cursor: u64, limit: usize) -> Result<OutputPage, String> {
        if cursor > self.end {
            return Err(format!(
                "output cursor {cursor} is ahead of this execution (end {})",
                self.end
            ));
        }
        if limit == 0 || limit > 8192 {
            return Err("output limit must be between 1 and 8192 bytes".into());
        }
        let start = self.start();
        let from = cursor.max(start);
        let mut bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip((from - start) as usize)
            .take(limit)
            .copied()
            .collect();
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_none() {
                bytes.truncate(error.valid_up_to());
            }
        }
        let next = from + bytes.len() as u64;
        Ok(OutputPage {
            bytes,
            cursor: next,
            end: self.end,
            truncated: cursor < start,
            has_more: next < self.end,
            matches: None,
        })
    }

    fn tail_start(&self, lines: usize) -> u64 {
        let last = self.bytes.len().saturating_sub(1);
        let mut seen = 0;
        for (index, byte) in self.bytes.iter().enumerate().rev() {
            if *byte == b'\n' && index != last {
                seen += 1;
                if seen == lines {
                    return self.start() + index as u64 + 1;
                }
            }
        }
        self.start()
    }

    fn raw_tail(&self, lines: usize, limit: usize) -> OutputPage {
        let wanted = self.tail_start(lines);
        let mut from = wanted.max(self.end.saturating_sub(limit as u64));
        while from < self.end && self.bytes[(from - self.start()) as usize] & 0xc0 == 0x80 {
            from += 1;
        }
        OutputPage {
            bytes: self.slice(from, self.end),
            cursor: self.end,
            end: self.end,
            truncated: from > wanted || (wanted == self.start() && self.start() > 0),
            has_more: false,
            matches: None,
        }
    }

    fn plain_page(&self, cursor: u64, limit: usize) -> OutputPage {
        let start = self.start();
        let from = cursor.max(start);
        let to = self.end.min(from + PLAIN_WINDOW);
        let reaches_end = to == self.end;
        let mut lines = terminal_text::render(&self.slice(from, to), from);
        if !reaches_end && lines.len() > 1 {
            lines.pop();
        }
        let lines = terminal_text::collapse(lines);
        let (text, taken) = fit_from_start(&lines, limit);
        let next = match taken {
            all if all == lines.len() && reaches_end => self.end,
            0 => to,
            some => lines[some - 1].end,
        };
        OutputPage {
            bytes: text.into_bytes(),
            cursor: next,
            end: self.end,
            truncated: cursor < start,
            has_more: next < self.end,
            matches: None,
        }
    }

    fn plain_tail(&self, lines: usize, limit: usize) -> OutputPage {
        let from = self.start().max(self.end.saturating_sub(PLAIN_WINDOW));
        let rendered =
            terminal_text::collapse(terminal_text::render(&self.slice(from, self.end), from));
        let wanted = &rendered[rendered.len().saturating_sub(lines)..];
        let (text, taken) = fit_from_end(wanted, limit);
        OutputPage {
            bytes: text.into_bytes(),
            cursor: self.end,
            end: self.end,
            truncated: taken < wanted.len() || (wanted.len() < lines && from > 0),
            has_more: false,
            matches: None,
        }
    }

    fn search(&self, needle: &str, query: &OutputQuery) -> Result<OutputPage, String> {
        if needle.is_empty() || needle.len() > 4096 {
            return Err("search must be nonempty text of at most 4096 bytes".into());
        }
        let start = self.start();
        let from = query.cursor.max(start);
        let lines = terminal_text::render(&self.slice(from, self.end), from);
        let needle = needle.to_lowercase();
        let hits: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect();
        let chosen = match query.tail {
            Some(count) => &hits[hits.len().saturating_sub(count)..],
            None => &hits[..],
        };
        let mut groups: Vec<(usize, usize)> = Vec::new();
        for &hit in chosen {
            let low = hit.saturating_sub(query.context);
            let high = (hit + query.context + 1).min(lines.len());
            match groups.last_mut() {
                Some(last) if low <= last.1 => last.1 = last.1.max(high),
                _ => groups.push((low, high)),
            }
        }
        let mut shown: Vec<terminal_text::Line> = Vec::new();
        for (low, high) in groups {
            if let Some(previous) = shown.last() {
                shown.push(terminal_text::Line {
                    text: "--".into(),
                    end: previous.end,
                });
            }
            shown.extend_from_slice(&lines[low..high]);
        }
        let page = if query.tail.is_some() {
            let (text, taken) = fit_from_end(&shown, query.limit);
            OutputPage {
                bytes: text.into_bytes(),
                cursor: self.end,
                end: self.end,
                truncated: taken < shown.len(),
                has_more: false,
                matches: Some(hits.len()),
            }
        } else {
            let (text, taken) = fit_from_start(&shown, query.limit);
            let next = if taken == shown.len() {
                self.end
            } else {
                shown[taken - 1].end
            };
            OutputPage {
                bytes: text.into_bytes(),
                cursor: next,
                end: self.end,
                truncated: query.cursor < start,
                has_more: next < self.end,
                matches: Some(hits.len()),
            }
        };
        Ok(page)
    }
}

fn fit_from_start(lines: &[terminal_text::Line], limit: usize) -> (String, usize) {
    let mut text = String::new();
    let mut taken = 0;
    for line in lines {
        if text.len() + line.text.len() + 1 > limit {
            break;
        }
        text.push_str(&line.text);
        text.push('\n');
        taken += 1;
    }
    if taken == 0 {
        if let Some(line) = lines.first() {
            let mut cut = limit.min(line.text.len());
            while !line.text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.push_str(&line.text[..cut]);
            taken = 1;
        }
    }
    (text, taken)
}

fn fit_from_end(lines: &[terminal_text::Line], limit: usize) -> (String, usize) {
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in lines.iter().rev() {
        if size + line.text.len() + 1 > limit {
            break;
        }
        size += line.text.len() + 1;
        kept.push(&line.text);
    }
    if kept.is_empty() {
        if let Some(line) = lines.last() {
            let mut cut = line.text.len().saturating_sub(limit);
            while !line.text.is_char_boundary(cut) {
                cut += 1;
            }
            return (line.text[cut..].to_string(), 1);
        }
    }
    let taken = kept.len();
    let mut text = String::with_capacity(size);
    for line in kept.into_iter().rev() {
        text.push_str(line);
        text.push('\n');
    }
    (text, taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restored_log_keeps_its_cursors() {
        let mut log = OutputLog::default();
        log.push(b"first ");
        log.push(b"second");
        let (bytes, end) = log.contents();
        let restored = OutputLog::restored(&bytes, end);
        assert_eq!(restored.read(6, 100).unwrap().bytes, b"second");
        assert_eq!(restored.read(0, 100).unwrap().end, 12);
    }

    #[test]
    fn output_pages_preserve_split_utf8() {
        let mut log = OutputLog::default();
        log.push("abcé".as_bytes());
        let first = log.read(0, 4).unwrap();
        assert_eq!(first.bytes, b"abc");
        assert_eq!(first.cursor, 3);
        assert_eq!(log.read(first.cursor, 4).unwrap().bytes, "é".as_bytes());
        log.push(&[0xe2, 0x82]);
        assert!(log.read(5, 8).unwrap().bytes.is_empty());
        log.push(&[0xac]);
        assert_eq!(log.read(5, 8).unwrap().bytes, "€".as_bytes());
    }

    #[test]
    fn output_cursors_page_without_repeating_and_report_eviction() {
        let mut log = OutputLog::default();
        log.push(b"abc");
        let page = log.read(0, 2).unwrap();
        assert_eq!(page.bytes, b"ab");
        assert!(page.has_more);
        assert_eq!(log.read(page.cursor, 2).unwrap().bytes, b"c");
        log.push(&vec![b'x'; MAX_OUTPUT + 5]);
        let page = log.read(0, 8192).unwrap();
        assert!(page.truncated);
        assert_eq!(page.cursor, 8 + 8192);
        assert!(log.read(u64::MAX, 1).is_err());
        assert!(log.read(0, 0).is_err());
    }

    fn query(log: &OutputLog, query: OutputQuery) -> (String, OutputPage) {
        let page = log
            .query(&OutputQuery {
                limit: 8192,
                ..query
            })
            .unwrap();
        (String::from_utf8(page.bytes.clone()).unwrap(), page)
    }

    #[test]
    fn every_page_and_the_ahead_error_report_the_end() {
        let mut log = OutputLog::default();
        log.push(b"hello\n");
        assert_eq!(log.read(0, 2).unwrap().end, 6);
        assert_eq!(
            log.read(99, 2).unwrap_err(),
            "output cursor 99 is ahead of this execution (end 6)"
        );
    }

    #[test]
    fn tail_reads_the_last_lines_within_the_limit() {
        let mut log = OutputLog::default();
        log.push(b"one\ntwo\nthree\nfour\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                tail: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(text, "three\nfour\n");
        assert_eq!(
            (page.cursor, page.has_more, page.truncated),
            (19, false, false)
        );
        let cut = log
            .query(&OutputQuery {
                tail: Some(3),
                limit: 6,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(cut.bytes, b"\nfour\n");
        assert!(cut.truncated);
        log.push("\u{e9}\u{e9}".as_bytes());
        let split = log
            .query(&OutputQuery {
                tail: Some(1),
                limit: 3,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(split.bytes, "\u{e9}".as_bytes());
        assert!(log
            .query(&OutputQuery {
                tail: Some(0),
                limit: 10,
                ..Default::default()
            })
            .is_err());
    }

    #[test]
    fn search_returns_matching_lines_with_context() {
        let mut log = OutputLog::default();
        log.push(b"a\nb\n\x1b[31mERROR\x1b[0m one\nc\nd\ne\nf\nerror two\ng\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                context: 1,
                ..Default::default()
            },
        );
        assert_eq!(text, "b\nERROR one\nc\n--\nf\nerror two\ng\n");
        assert_eq!((page.matches, page.has_more), (Some(2), false));
        let (latest, _) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                tail: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(latest, "error two\n");
        let first = log
            .query(&OutputQuery {
                search: Some("error".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.bytes, b"ERROR one\n");
        assert!(first.has_more);
        let (rest, _) = query(
            &log,
            OutputQuery {
                search: Some("error".into()),
                cursor: first.cursor,
                ..Default::default()
            },
        );
        assert_eq!(rest, "error two\n");
    }

    #[test]
    fn plain_pages_replay_redraws_and_collapse_repeats() {
        let mut log = OutputLog::default();
        log.push(b"\x1b[1mstep 1/2\x1b[0m\rstep 2/2\n");
        log.push(&b"same\n".repeat(50));
        log.push(b"done\n");
        let (text, page) = query(
            &log,
            OutputQuery {
                plain: true,
                ..Default::default()
            },
        );
        assert_eq!(text, "step 2/2\nsame\n[repeated 49 more times]\ndone\n");
        assert_eq!((page.cursor, page.has_more), (log.end, false));
        let (tail, _) = query(
            &log,
            OutputQuery {
                plain: true,
                tail: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(tail, "[repeated 49 more times]\ndone\n");
        let first = log
            .query(&OutputQuery {
                plain: true,
                limit: 14,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(first.bytes, b"step 2/2\nsame\n");
        assert!(first.has_more);
        let (rest, _) = query(
            &log,
            OutputQuery {
                plain: true,
                cursor: first.cursor,
                ..Default::default()
            },
        );
        assert_eq!(rest, "same\n[repeated 48 more times]\ndone\n");
    }

    #[test]
    fn plain_reads_do_not_loop_on_trailing_escape_codes() {
        let mut log = OutputLog::default();
        log.push(b"ready\n\x1b[0m");
        let page = log
            .query(&OutputQuery {
                plain: true,
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.bytes, b"ready\n");
        assert_eq!((page.cursor, page.has_more), (log.end, false));
    }
}
