//! Everything a chat said, on disk, one event a line. The replay in memory
//! keeps only the newest part of a long chat; a phone that scrolls back past
//! it reads the rest from here, a page of whole turns at a time.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use sikemux_fold::{ChatEvent, ChatEventKind};
use sikemux_process::files::{create_private, private_folder, private_options};

/// Past this the oldest half of the file goes, so a chat that never ends
/// cannot fill the disk. Its earliest turns are then out of reach.
pub const MAX_HISTORY_BYTES: u64 = 512 * 1024 * 1024;
const ZST_LEVEL: i32 = 3;

/// One line of the file: an event and where it stands, so a core that
/// starts after this one can read the file back.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Line<E> {
    index: u64,
    /// The number of the last change the event made in the chat; 0 when written without one.
    #[serde(default)]
    seq: u64,
    starts_turn: bool,
    point: bool,
    event: E,
}

/// An event read back from a file an earlier core left.
pub struct Recovered {
    pub index: u64,
    pub seq: u64,
    pub starts_turn: bool,
    pub point: bool,
    pub bytes: usize,
    pub event: ChatEvent,
}

/// An event a page can start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Point {
    index: u64,
    offset: u64,
    starts_turn: bool,
}

enum Backing {
    None,
    Plain(File),
    Compressed { path: PathBuf, disk_bytes: u64 },
}

pub struct History {
    path: PathBuf,
    backing: Backing,
    bytes: u64,
    max_bytes: u64,
    /// The index of the first event still in the file.
    first: u64,
    /// One past the index of the last event written.
    next: u64,
    points: Vec<Point>,
    /// Counts the times the file was rewritten, cut back or given up, so a page read outside
    /// the chat's lock can tell its file changed under it.
    rewrites: Arc<AtomicU64>,
    /// The chat is closed or asleep and can be opened again, so the file stays when it is let go.
    outlives_chat: bool,
}

enum ReaderSource {
    Plain(File),
    Compressed(PathBuf),
}

/// What a page needs of a history file, taken under the chat's lock so the reading itself can
/// happen outside it while the chat goes on streaming.
pub struct Reader {
    source: ReaderSource,
    first: u64,
    next: u64,
    bytes: u64,
    points: Vec<Point>,
    rewrites: Arc<AtomicU64>,
    taken_at: u64,
}

/// A history file let go of whole, for a core that takes the chat over to
/// go on writing.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryState {
    path: PathBuf,
    bytes: u64,
    max_bytes: u64,
    first: u64,
    next: u64,
    points: Vec<Point>,
}

/// Events from before some index, and where the page before them starts.
#[derive(Debug, Default, PartialEq)]
pub struct Page {
    /// Each with its index.
    pub events: Vec<(u64, ChatEvent)>,
    pub older_before: Option<u64>,
    /// The index of the page's first event.
    pub first: u64,
}

/// A prompt the person sent, as the history keeps it.
pub struct PersonSaid {
    pub index: u64,
    /// When it was sent, in Unix milliseconds, where the history says.
    pub at: Option<u64>,
    pub text: String,
    /// Each agent the person named with `@`, as the mention read and the agent it links to.
    pub mentions: Vec<(String, String)>,
}

fn mentions(payload: &serde_json::Value) -> Vec<(String, String)> {
    (payload
        .get("context")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten())
    .filter_map(|item| {
        let agent = item
            .get("uri")?
            .as_str()?
            .strip_prefix(sikemux_view::chat::AGENT_URI)?;
        let title = item.get("title")?.as_str()?;
        Some((title.to_owned(), agent.to_owned()))
    })
    .collect()
}

/// The prompts the person sent in the history file `name` in `dir`, newest first, from its last
/// `max_bytes`: only those the core marked as the person's, as they typed them.
pub fn persons_prompts(dir: &Path, name: &str, max_bytes: u64) -> Vec<PersonSaid> {
    let mut said: Vec<PersonSaid> = (tail(dir, name, max_bytes).into_iter())
        .filter_map(|(index, event)| {
            let payload = &event.payload;
            let person = payload.get("person").and_then(serde_json::Value::as_bool) == Some(true);
            let text = payload.get("text").and_then(serde_json::Value::as_str)?;
            (event.kind == ChatEventKind::Prompt && person).then(|| PersonSaid {
                index,
                at: payload.get("at").and_then(serde_json::Value::as_u64),
                text: text.to_owned(),
                mentions: mentions(payload),
            })
        })
        .collect();
    said.reverse();
    said
}

/// The prompt the person sent that started the latest turn of the history file `name` in `dir`,
/// read from its last `max_bytes`: the first of theirs since the turn before ended, by its own
/// end or by a failure. None when the person's words did not start it, as when an agent or
/// Sikemux did.
pub fn turn_prompt(dir: &Path, name: &str, max_bytes: u64) -> Option<PersonSaid> {
    let events = tail(dir, name, max_bytes);
    let since = (events.iter())
        .rposition(|(_, event)| sikemux_fold::update::ends_turn(event.kind, &event.payload))
        .map_or(0, |ended| ended + 1);
    let (index, event) = (events.get(since..)?.iter()).find(|(_, event)| {
        event.kind == ChatEventKind::Prompt
            && event
                .payload
                .get("person")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    })?;
    let payload = &event.payload;
    Some(PersonSaid {
        index: *index,
        at: payload.get("at").and_then(serde_json::Value::as_u64),
        text: payload.get("text")?.as_str()?.to_owned(),
        mentions: mentions(payload),
    })
}

fn plain_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.jsonl"))
}

fn compressed_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.jsonl.zst"))
}

fn zst_of(plain: &Path) -> PathBuf {
    let mut path = plain.as_os_str().to_os_string();
    path.push(".zst");
    PathBuf::from(path)
}

fn feed_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".jsonl.zst")
        .or_else(|| name.strip_suffix(".jsonl"))
        .or_else(|| path.file_stem()?.to_str())
}

pub fn exists(dir: &Path, name: &str) -> bool {
    plain_path(dir, name).is_file() || compressed_path(dir, name).is_file()
}

pub fn remove(dir: &Path, name: &str) {
    let _ = fs::remove_file(plain_path(dir, name));
    let _ = fs::remove_file(compressed_path(dir, name));
}

pub fn compress_file(dir: &Path, name: &str) -> io::Result<Option<u64>> {
    let plain = plain_path(dir, name);
    let zst = compressed_path(dir, name);
    if let Ok(meta) = fs::metadata(&plain) {
        if meta.is_file() && meta.len() > 0 {
            let disk_bytes = encode_zst(&plain, &zst)?;
            let _ = fs::remove_file(&plain);
            return Ok(Some(disk_bytes));
        }
    }
    if let Ok(meta) = fs::metadata(&zst) {
        if meta.is_file() {
            return Ok(Some(meta.len()));
        }
    }
    Ok(None)
}

fn encode_zst(plain: &Path, zst: &Path) -> io::Result<u64> {
    let tmp = zst.with_extension("zst.tmp");
    let encoded = (|| -> io::Result<u64> {
        let mut from = File::open(plain)?;
        let mut to = create_private(&tmp)?;
        zstd::stream::copy_encode(&mut from, &mut to, ZST_LEVEL)?;
        to.flush()?;
        drop(to);
        fs::rename(&tmp, zst)?;
        Ok(fs::metadata(zst)?.len())
    })();
    if encoded.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    encoded
}

fn decode_zst(zst: &Path, plain: &Path) -> io::Result<()> {
    let tmp = plain.with_extension("jsonl.tmp");
    let decoded = (|| -> io::Result<()> {
        let mut from = File::open(zst)?;
        let mut to = create_private(&tmp)?;
        zstd::stream::copy_decode(&mut from, &mut to)?;
        to.flush()?;
        drop(to);
        fs::rename(&tmp, plain)
    })();
    if decoded.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    decoded
}

/// The events of the history file `name` in `dir` in its last `max_bytes`, oldest first, by
/// index. Reads the file alone, never the chat's lock, so a chat streaming meanwhile is not held
/// up; an event still being written is not there yet.
pub fn tail(dir: &Path, name: &str, max_bytes: u64) -> Vec<(u64, ChatEvent)> {
    let path = plain_path(dir, name);
    if let Ok(mut file) = File::open(&path) {
        let len = file.metadata().map_or(0, |meta| meta.len());
        let start = len.saturating_sub(max_bytes);
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        if start > 0 && reader.read_until(b'\n', &mut line).is_err() {
            return Vec::new();
        }
        let mut events = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(read) if read > 0 && line.last() == Some(&b'\n') => {}
                _ => break,
            }
            if let Ok(kept) = serde_json::from_slice::<Line<ChatEvent>>(&line) {
                events.push((kept.index, kept.event));
            }
        }
        return events;
    }
    let zst = compressed_path(dir, name);
    let Ok(file) = File::open(&zst) else {
        return Vec::new();
    };
    let Ok(decoder) = zstd::stream::read::Decoder::new(file) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(decoder);
    let mut line = Vec::new();
    let mut kept: VecDeque<(usize, u64, ChatEvent)> = VecDeque::new();
    let mut total: u64 = 0;
    loop {
        line.clear();
        let read = match reader.read_until(b'\n', &mut line) {
            Ok(read) if read > 0 && line.last() == Some(&b'\n') => read,
            _ => break,
        };
        if let Ok(parsed) = serde_json::from_slice::<Line<ChatEvent>>(&line) {
            total += read as u64;
            kept.push_back((read, parsed.index, parsed.event));
            while total > max_bytes && kept.len() > 1 {
                if let Some((old, ..)) = kept.pop_front() {
                    total = total.saturating_sub(old as u64);
                }
            }
        }
    }
    kept.into_iter()
        .map(|(_, index, event)| (index, event))
        .collect()
}

/// Removes what an earlier core left, but for the files of chats `keep`
/// names, which this core took over.
pub fn clear(dir: &Path, keep: &[String]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let kept = feed_stem(&path).is_some_and(|stem| keep.iter().any(|name| name == stem));
        if !kept {
            let _ = fs::remove_file(path);
        }
    }
}

impl History {
    pub fn open(dir: &Path, name: &str, max_bytes: u64) -> Self {
        let path = plain_path(dir, name);
        let file = private_folder(dir)
            .and_then(|()| {
                private_options()
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
            backing: match file {
                Some(file) => Backing::Plain(file),
                None => Backing::None,
            },
            bytes: 0,
            max_bytes,
            first: 0,
            next: 0,
            points: Vec::new(),
            rewrites: Arc::default(),
            outlives_chat: false,
        }
    }

    /// Goes on writing the file an earlier core left when it stopped, telling
    /// `each` every event in it, oldest first. A line that core was cut off
    /// writing is dropped.
    pub fn recover(
        dir: &Path,
        name: &str,
        max_bytes: u64,
        mut each: impl FnMut(Recovered),
    ) -> Self {
        let path = plain_path(dir, name);
        match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(mut file) => {
                let mut history = Self {
                    path,
                    backing: Backing::None,
                    bytes: 0,
                    max_bytes,
                    first: 0,
                    next: 0,
                    points: Vec::new(),
                    rewrites: Arc::default(),
                    outlives_chat: false,
                };
                history.scan_lines(BufReader::new(&file), &mut each);
                let cut = file
                    .set_len(history.bytes)
                    .and_then(|()| file.seek(SeekFrom::Start(history.bytes)).map(drop));
                history.backing = Backing::Plain(file);
                if let Err(error) = cut {
                    history.fail(&error);
                }
                history
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let zst = compressed_path(dir, name);
                let Ok(file) = File::open(&zst) else {
                    return Self::open(dir, name, max_bytes);
                };
                let disk_bytes = file.metadata().map_or(0, |meta| meta.len());
                let Ok(decoder) = zstd::stream::read::Decoder::new(file) else {
                    return Self::open(dir, name, max_bytes);
                };
                let mut history = Self {
                    path,
                    backing: Backing::Compressed {
                        path: zst,
                        disk_bytes,
                    },
                    bytes: 0,
                    max_bytes,
                    first: 0,
                    next: 0,
                    points: Vec::new(),
                    rewrites: Arc::default(),
                    outlives_chat: false,
                };
                history.scan_lines(BufReader::new(decoder), &mut each);
                history
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: a chat's history at {} could not be read again: {error}",
                    path.display()
                );
                Self::open(dir, name, max_bytes)
            }
        }
    }

    fn scan_lines(&mut self, mut reader: impl BufRead, each: &mut impl FnMut(Recovered)) {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = match reader.read_until(b'\n', &mut line) {
                Ok(read) if line.last() == Some(&b'\n') => read,
                _ => break,
            };
            let Ok(kept) = serde_json::from_slice::<Line<ChatEvent>>(&line) else {
                break;
            };
            let offset = self.bytes;
            if offset == 0 {
                self.first = kept.index;
            }
            if kept.point || offset == 0 {
                self.points.push(Point {
                    index: kept.index,
                    offset,
                    starts_turn: kept.starts_turn || offset == 0,
                });
            }
            self.bytes += read as u64;
            self.next = kept.index + 1;
            each(Recovered {
                index: kept.index,
                seq: kept.seq,
                starts_turn: kept.starts_turn,
                point: kept.point,
                bytes: read,
                event: kept.event,
            });
        }
    }

    /// Compresses a resting history file with zstd and answers how many bytes it holds on disk.
    pub fn compress(&mut self) -> u64 {
        if self.bytes == 0 {
            return self.size();
        }
        let Backing::Plain(_) = self.backing else {
            return self.size();
        };
        let zst = zst_of(&self.path);
        self.backing = Backing::None;
        match encode_zst(&self.path, &zst) {
            Ok(disk_bytes) => {
                let _ = fs::remove_file(&self.path);
                self.backing = Backing::Compressed {
                    path: zst,
                    disk_bytes,
                };
                self.rewrites.fetch_add(1, Ordering::SeqCst);
                disk_bytes
            }
            Err(error) => {
                eprintln!(
                    "sikemux core: a resting chat's history at {} was left uncompressed: {error}",
                    self.path.display()
                );
                if let Ok(mut file) = OpenOptions::new().read(true).write(true).open(&self.path) {
                    let _ = file.seek(SeekFrom::End(0));
                    self.backing = Backing::Plain(file);
                }
                self.size()
            }
        }
    }

    /// Expands a compressed history file back to plain JSONL so a waking chat can append to it.
    pub fn decompress(&mut self) {
        let _ = self.ensure_plain();
    }

    fn ensure_plain(&mut self) -> bool {
        match &self.backing {
            Backing::Plain(_) => return true,
            Backing::None => return false,
            Backing::Compressed { .. } => {}
        }
        let Backing::Compressed { path: zst, .. } =
            std::mem::replace(&mut self.backing, Backing::None)
        else {
            return false;
        };
        let reopened = (|| -> io::Result<File> {
            decode_zst(&zst, &self.path)?;
            let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
            file.seek(SeekFrom::End(0))?;
            Ok(file)
        })();
        match reopened {
            Ok(file) => {
                let _ = fs::remove_file(&zst);
                self.backing = Backing::Plain(file);
                self.rewrites.fetch_add(1, Ordering::SeqCst);
                true
            }
            Err(error) => {
                self.backing = Backing::Compressed {
                    disk_bytes: fs::metadata(&zst).map_or(0, |meta| meta.len()),
                    path: zst,
                };
                self.fail(&error);
                false
            }
        }
    }

    /// Lets go of the file without removing it.
    pub fn release(mut self) -> HistoryState {
        let _ = self.ensure_plain();
        let file = match std::mem::replace(&mut self.backing, Backing::None) {
            Backing::Plain(file) => Some(file),
            Backing::Compressed { .. } | Backing::None => None,
        };
        let state = HistoryState {
            path: self.path.clone(),
            bytes: if file.is_some() { self.bytes } else { 0 },
            max_bytes: self.max_bytes,
            first: self.first,
            next: self.next,
            points: std::mem::take(&mut self.points),
        };
        if file.is_none() {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_file(zst_of(&self.path));
        }
        state
    }

    /// Goes on writing a file an earlier core let go of.
    pub fn reopen(state: HistoryState) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&state.path)
            .and_then(|mut file| file.seek(SeekFrom::Start(state.bytes)).map(|_| file))
            .inspect_err(|error| {
                eprintln!(
                    "sikemux core: a chat's history at {} was not kept through the update: {error}",
                    state.path.display()
                )
            })
            .ok();
        let kept = file.is_some();
        Self {
            path: state.path,
            backing: match file {
                Some(file) => Backing::Plain(file),
                None => Backing::None,
            },
            bytes: if kept { state.bytes } else { 0 },
            max_bytes: state.max_bytes,
            first: state.first,
            next: state.next,
            points: if kept { state.points } else { Vec::new() },
            rewrites: Arc::default(),
            outlives_chat: false,
        }
    }

    /// Whether the file stays on disk once the chat lets go of it.
    pub fn outlive_chat(&mut self, outlives: bool) {
        self.outlives_chat = outlives;
    }

    /// How many bytes the file holds on disk.
    pub fn size(&self) -> u64 {
        match &self.backing {
            Backing::Plain(_) => self.bytes,
            Backing::Compressed { disk_bytes, .. } => *disk_bytes,
            Backing::None => 0,
        }
    }

    /// The index of the oldest event a page can still return.
    pub fn first(&self) -> Option<u64> {
        (!matches!(self.backing, Backing::None)).then_some(self.first)
    }

    pub fn append(
        &mut self,
        index: u64,
        seq: u64,
        event: &ChatEvent,
        point: bool,
        starts_turn: bool,
    ) {
        if !self.ensure_plain() {
            return;
        }
        if self.bytes == 0 {
            self.first = index;
        }
        let Ok(mut line) = serde_json::to_vec(&Line {
            index,
            seq,
            starts_turn,
            point,
            event,
        }) else {
            return;
        };
        line.push(b'\n');
        let offset = self.bytes;
        let written = match &mut self.backing {
            Backing::Plain(file) => file.write_all(&line),
            Backing::Compressed { .. } | Backing::None => Ok(()),
        };
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
        self.backing = Backing::None;
        self.points.clear();
        self.rewrites.fetch_add(1, Ordering::SeqCst);
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(zst_of(&self.path));
    }

    fn drop_oldest_half(&mut self) -> io::Result<()> {
        self.rewrites.fetch_add(1, Ordering::SeqCst);
        let half = self.bytes / 2;
        let after_half = |point: &&Point| point.offset >= half && point.offset > 0;
        let cut = self
            .points
            .iter()
            .find(|point| point.starts_turn && after_half(point))
            .or_else(|| self.points.iter().find(after_half))
            .copied();
        let Some(cut) = cut else {
            if let Backing::Plain(file) = &mut self.backing {
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
            let mut to = create_private(&kept)?;
            io::copy(&mut from, &mut to)?;
        }
        fs::rename(&kept, &self.path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        self.backing = Backing::Plain(file);
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

    /// Puts `events`, each with its index and whether a page or a turn starts there, before
    /// everything the file holds, as a chat reads further back into a session it took up.
    /// Reading them is change `seq` of the chat.
    pub fn prepend(&mut self, events: &[(u64, ChatEvent, bool, bool)], seq: u64) {
        let Some(&(first, ..)) = events.first() else {
            return;
        };
        if !self.ensure_plain() {
            return;
        }
        self.rewrites.fetch_add(1, Ordering::SeqCst);
        let mut text = Vec::new();
        let mut points = Vec::new();
        for (index, event, point, starts_turn) in events {
            let offset = text.len() as u64;
            let Ok(line) = serde_json::to_vec(&Line {
                index: *index,
                seq,
                starts_turn: *starts_turn,
                point: *point,
                event,
            }) else {
                continue;
            };
            text.extend(line);
            text.push(b'\n');
            if *point || offset == 0 {
                points.push(Point {
                    index: *index,
                    offset,
                    starts_turn: *starts_turn || offset == 0,
                });
            }
        }
        let added = text.len() as u64;
        let written = self.rewrite_with(&text);
        if let Err(error) = written {
            self.fail(&error);
            return;
        }
        self.bytes += added;
        self.first = first;
        for point in &mut self.points {
            point.offset += added;
        }
        points.append(&mut self.points);
        self.points = points;
    }

    /// The file again with `head` before what it holds, written beside it and moved over it.
    fn rewrite_with(&mut self, head: &[u8]) -> io::Result<()> {
        let fresh = self.path.with_extension("jsonl.earlier");
        {
            let mut to = create_private(&fresh)?;
            to.write_all(head)?;
            io::copy(&mut File::open(&self.path)?, &mut to)?;
        }
        fs::rename(&fresh, &self.path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        self.backing = Backing::Plain(file);
        Ok(())
    }

    /// Forgets the event at `index` and everything after it. One that does
    /// not start a page cannot be found in the file, so everything goes.
    pub fn truncate(&mut self, index: u64) {
        if index >= self.next || !self.ensure_plain() {
            return;
        }
        self.rewrites.fetch_add(1, Ordering::SeqCst);
        let offset = match self.points.iter().find(|point| point.index == index) {
            Some(point) if index > self.first => point.offset,
            _ => 0,
        };
        let cut = match &mut self.backing {
            Backing::Plain(file) => file
                .set_len(offset)
                .and_then(|()| file.seek(SeekFrom::Start(offset)).map(drop)),
            Backing::Compressed { .. } | Backing::None => Ok(()),
        };
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

    /// The newest event that starts a page and that `picks`, by its index.
    pub fn find(&self, picks: impl Fn(&ChatEvent) -> bool) -> Option<u64> {
        match &self.backing {
            Backing::None => None,
            Backing::Plain(_) => {
                let mut reader = BufReader::new(File::open(&self.path).ok()?);
                let mut line = Vec::new();
                for point in self.points.iter().rev() {
                    reader.seek(SeekFrom::Start(point.offset)).ok()?;
                    line.clear();
                    reader.read_until(b'\n', &mut line).ok()?;
                    if serde_json::from_slice::<Line<ChatEvent>>(&line)
                        .is_ok_and(|line| picks(&line.event))
                    {
                        return Some(point.index);
                    }
                }
                None
            }
            Backing::Compressed { path, .. } => {
                let decoder = zstd::stream::read::Decoder::new(File::open(path).ok()?).ok()?;
                let mut reader = BufReader::new(decoder);
                let mut line = Vec::new();
                let mut offset = 0u64;
                let mut next_point = 0usize;
                let mut found = None;
                while let Some(point) = self.points.get(next_point) {
                    line.clear();
                    let read = reader.read_until(b'\n', &mut line).ok()?;
                    if read == 0 {
                        break;
                    }
                    if offset == point.offset {
                        if serde_json::from_slice::<Line<ChatEvent>>(&line)
                            .is_ok_and(|line| picks(&line.event))
                        {
                            found = Some(point.index);
                        }
                        next_point += 1;
                    } else if offset > point.offset {
                        next_point += 1;
                    }
                    offset += read as u64;
                }
                found
            }
        }
    }

    /// What a page read needs of the file, to read outside the chat's lock; none while no file
    /// is kept.
    pub fn reader(&self) -> io::Result<Option<Reader>> {
        let source = match &self.backing {
            Backing::None => return Ok(None),
            Backing::Plain(_) => ReaderSource::Plain(File::open(&self.path)?),
            Backing::Compressed { path, .. } => ReaderSource::Compressed(path.clone()),
        };
        Ok(Some(Reader {
            source,
            first: self.first,
            next: self.next,
            bytes: self.bytes,
            points: self.points.clone(),
            rewrites: self.rewrites.clone(),
            taken_at: self.rewrites.load(Ordering::SeqCst),
        }))
    }
}

impl Reader {
    /// Whether the file is still as it was when the reader was taken: nothing was cut back or
    /// rewritten since, so what was read from it holds.
    pub fn still_current(&self) -> bool {
        self.rewrites.load(Ordering::SeqCst) == self.taken_at
    }

    /// The index of the first event the file still holds, and one past its last.
    pub fn span(&self) -> (u64, u64) {
        (self.first, self.next)
    }

    /// How many bytes the file holds.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Each event a page can start from, oldest first: its index, where it sits in the file,
    /// and whether it starts a turn.
    pub fn points(&self) -> impl Iterator<Item = (u64, u64, bool)> + '_ {
        self.points
            .iter()
            .map(|point| (point.index, point.offset, point.starts_turn))
    }

    /// The events from `lo` up to `hi`, each with its index, read from the page point at or
    /// before `lo`. Permission requests are left out: they were answered long ago.
    pub fn range(&self, lo: u64, hi: u64) -> io::Result<Vec<(u64, ChatEvent)>> {
        Ok(self
            .lines(lo, hi)?
            .into_iter()
            .map(|line| (line.index, line.event))
            .collect())
    }

    /// [`Reader::range`] with where each event stands, for a replay to hold them again.
    pub fn lines(&self, lo: u64, hi: u64) -> io::Result<Vec<Recovered>> {
        let lo = lo.max(self.first);
        let hi = hi.min(self.next);
        if lo >= hi {
            return Ok(Vec::new());
        }
        let from = self
            .points
            .partition_point(|point| point.index <= lo)
            .checked_sub(1)
            .and_then(|last| self.points.get(last))
            .map_or(0, |point| point.offset);
        let to = self
            .points
            .iter()
            .find(|point| point.index >= hi)
            .map_or(self.bytes, |point| point.offset);
        let len = to.saturating_sub(from);
        let text = match &self.source {
            ReaderSource::Plain(file) => {
                let mut text = vec![0; len as usize];
                crate::os::read_exact_at(file, &mut text, from)?;
                text
            }
            ReaderSource::Compressed(path) => {
                let mut decoder = zstd::stream::read::Decoder::new(File::open(path)?)?;
                if from > 0 {
                    io::copy(&mut decoder.by_ref().take(from), &mut io::sink())?;
                }
                let mut text = Vec::with_capacity(len as usize);
                decoder.take(len).read_to_end(&mut text)?;
                text
            }
        };
        Ok(text
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|text| {
                let line = serde_json::from_slice::<Line<ChatEvent>>(text).ok()?;
                Some(Recovered {
                    index: line.index,
                    seq: line.seq,
                    starts_turn: line.starts_turn,
                    point: line.point,
                    bytes: text.len() + 1,
                    event: line.event,
                })
            })
            .filter(|line| (lo..hi).contains(&line.index))
            .filter(|line| line.event.kind != ChatEventKind::PermissionRequest)
            .collect())
    }
}

impl Drop for History {
    fn drop(&mut self) {
        if !self.outlives_chat {
            match std::mem::replace(&mut self.backing, Backing::None) {
                Backing::Plain(_) => {
                    let _ = fs::remove_file(&self.path);
                }
                Backing::Compressed { path, .. } => {
                    let _ = fs::remove_file(path);
                }
                Backing::None => {}
            }
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

    fn read(history: &History, lo: u64, hi: u64) -> Vec<u64> {
        history
            .reader()
            .unwrap()
            .map(|reader| reader.range(lo, hi).unwrap())
            .unwrap_or_default()
            .iter()
            .map(|(_, event)| event.payload["n"].as_u64().unwrap_or(u64::MAX))
            .collect()
    }

    /// Three events a turn: the turn starting, a reply, and the turn ending.
    fn turns(history: &mut History, count: u64) {
        for turn in 0..count {
            let base = turn * 3;
            history.append(
                base,
                base,
                &event(ChatEventKind::TurnStarted, base),
                true,
                true,
            );
            history.append(
                base + 1,
                base + 1,
                &event(ChatEventKind::Error, base + 1),
                false,
                false,
            );
            history.append(
                base + 2,
                base + 2,
                &event(ChatEventKind::TurnCompleted, base + 2),
                false,
                false,
            );
        }
    }

    #[test]
    fn reads_any_stretch_of_the_chat_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 5);
        assert_eq!(read(&history, 3, 9), [3, 4, 5, 6, 7, 8]);
        assert_eq!(read(&history, 4, 7), [4, 5, 6]);
        assert_eq!(read(&history, 12, 100), [12, 13, 14]);
        assert!(read(&history, 9, 9).is_empty());
    }

    #[test]
    fn a_rewind_forgets_from_its_turn_and_keeps_writing_after() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 4);
        history.truncate(6);
        assert_eq!(read(&history, 0, 100), [0, 1, 2, 3, 4, 5]);
        history.append(20, 20, &event(ChatEventKind::TurnStarted, 20), true, true);
        assert_eq!(read(&history, 0, 100), [0, 1, 2, 3, 4, 5, 20]);
    }

    #[test]
    fn a_rewind_to_an_event_that_starts_no_page_forgets_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 3);
        history.truncate(4);
        assert!(read(&history, 0, 100).is_empty());
    }

    #[test]
    fn answered_permission_requests_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        history.append(0, 0, &event(ChatEventKind::TurnStarted, 0), true, true);
        history.append(
            1,
            1,
            &event(ChatEventKind::PermissionRequest, 1),
            false,
            false,
        );
        history.append(2, 2, &event(ChatEventKind::TurnCompleted, 2), false, false);
        assert_eq!(read(&history, 0, 3), [0, 2]);
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
        assert_eq!(read(&history, 0, 30), (first..30).collect::<Vec<_>>());
    }

    #[test]
    fn the_file_goes_with_the_chat_unless_the_chat_can_be_opened_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::open(dir.path(), "chat", MAX_HISTORY_BYTES);
        turns(&mut history, 1);
        let path = history.path.clone();
        assert!(path.exists());
        drop(history);
        assert!(!path.exists());

        let mut history = History::open(dir.path(), "closed", MAX_HISTORY_BYTES);
        turns(&mut history, 1);
        history.outlive_chat(true);
        let path = history.path.clone();
        drop(history);
        assert!(path.exists());
    }
}
