//! The Program Status Protocol, OSC 7501: a program in the terminal says
//! itself whether it is idle, working, blocked on the person, done or failed.
//! Spec: https://www.superlogical.com/rex/docs/build/program-status

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;

const PREFIX: &[u8] = b"7501;";
const MAX_SEQUENCE_BYTES: usize = 4096;
const MAX_KEY_BYTES: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE_DECODED: usize = 192;
const MAX_APP_BYTES: usize = 32;
const MAX_ID_BYTES: usize = 128;
const MAX_ID_SEGMENT_BYTES: usize = 32;
const MAX_ID_DEPTH: usize = 8;
const MAX_RECORDS: usize = 64;
pub const FEATURE_REPLY: &[u8] = b"\x1b]7501;?\x1b\\";

const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportedState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl ReportedState {
    pub fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    fn lasts_past_prompt(self) -> bool {
        matches!(self, Self::Idle | Self::Done | Self::Error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedOn {
    Permission,
    Question,
    Auth,
}

impl BlockedOn {
    pub fn word(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// What the program last said about one piece of its work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRecord {
    /// Empty for the program's own record; `build/test` for a worker under `build`.
    pub id: String,
    pub state: ReportedState,
    pub blocked_on: Option<BlockedOn>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
    touched: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scan {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

#[derive(Default)]
pub struct ProcessedStatus {
    /// The records changed.
    pub changed: bool,
    /// The program asked whether the terminal reads the protocol.
    pub asked: bool,
}

/// Reads OSC 7501 reports out of a terminal's output and keeps the records
/// they leave.
#[derive(Default)]
pub struct ProgramStatus {
    scan: Scan,
    osc: Vec<u8>,
    overflowed: bool,
    records: Vec<StatusRecord>,
    clock: u64,
}

impl ProgramStatus {
    pub fn records(&self) -> &[StatusRecord] {
        &self.records
    }

    /// The record that matters most to the person: anything blocked, then
    /// anything working, then the program's own record, then the newest.
    pub fn headline(&self) -> Option<&StatusRecord> {
        let newest = |state: Option<ReportedState>| {
            self.records
                .iter()
                .filter(|record| state.is_none_or(|state| record.state == state))
                .max_by_key(|record| record.touched)
        };
        newest(Some(ReportedState::Blocked))
            .or_else(|| newest(Some(ReportedState::Working)))
            .or_else(|| self.records.iter().find(|record| record.id.is_empty()))
            .or_else(|| newest(None))
    }

    pub fn process(&mut self, bytes: &[u8]) -> ProcessedStatus {
        let mut out = ProcessedStatus::default();
        let mut rest = bytes;
        while let Some((&byte, after)) = rest.split_first() {
            match self.scan {
                Scan::Ground => {
                    let Some(escape) = memchr::memchr_iter(b'\x1b', rest)
                        .find(|&at| matches!(rest.get(at + 1), Some(b']' | b'c' | b'\x1b') | None))
                    else {
                        break;
                    };
                    self.scan = Scan::Escape;
                    rest = &rest[escape + 1..];
                    continue;
                }
                Scan::Escape => match byte {
                    b']' => {
                        self.osc.clear();
                        self.overflowed = false;
                        self.scan = Scan::Osc;
                    }
                    b'c' => {
                        out.changed |= !self.records.is_empty();
                        self.records.clear();
                        self.scan = Scan::Ground;
                    }
                    b'\x1b' => {}
                    _ => self.scan = Scan::Ground,
                },
                Scan::Osc => {
                    let Some(end) = memchr::memchr2(b'\x07', b'\x1b', rest) else {
                        self.push(rest);
                        break;
                    };
                    self.push(&rest[..end]);
                    if rest[end] == b'\x07' {
                        self.finish(&mut out);
                    } else {
                        self.scan = Scan::OscEscape;
                    }
                    rest = &rest[end + 1..];
                    continue;
                }
                Scan::OscEscape => match byte {
                    b'\\' | b'\x07' => self.finish(&mut out),
                    b'\x1b' => self.push(b"\x1b"),
                    _ => {
                        self.push(&[b'\x1b', byte]);
                        self.scan = Scan::Osc;
                    }
                },
            }
            rest = after;
        }
        out
    }

    fn push(&mut self, bytes: &[u8]) {
        let room = MAX_SEQUENCE_BYTES.saturating_sub(self.osc.len());
        if bytes.len() > room {
            self.overflowed = true;
        }
        self.osc.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    fn finish(&mut self, out: &mut ProcessedStatus) {
        if !self.overflowed {
            let osc = std::mem::take(&mut self.osc);
            if osc == b"133;A" || osc.starts_with(b"133;A;") {
                out.changed |= self.drop_running();
            } else if let Some(body) = osc.strip_prefix(PREFIX) {
                if body == b"?" {
                    out.asked = true;
                } else if let Some(report) = parse_report(body) {
                    self.apply(report);
                    out.changed = true;
                }
            }
            self.osc = osc;
        }
        self.osc.clear();
        self.overflowed = false;
        self.scan = Scan::Ground;
    }

    /// A new shell prompt means the program that was running has finished.
    fn drop_running(&mut self) -> bool {
        let before = self.records.len();
        self.records
            .retain(|record| record.state.lasts_past_prompt());
        self.records.len() != before
    }

    fn apply(&mut self, report: Report) {
        let Some(mut record) = report.record else {
            self.records
                .retain(|record| !(report.id.is_empty() || within(&record.id, &report.id)));
            return;
        };
        self.clock += 1;
        record.touched = self.clock;
        if let Some(existing) = self.records.iter_mut().find(|old| old.id == record.id) {
            *existing = record;
            return;
        }
        if self.records.len() >= MAX_RECORDS {
            if let Some(stalest) = self
                .records
                .iter()
                .enumerate()
                .min_by_key(|(_, old)| old.touched)
                .map(|(index, _)| index)
            {
                self.records.swap_remove(stalest);
            }
        }
        self.records.push(record);
    }

    /// The app a record names, or the one its nearest ancestor names.
    pub fn app_of<'a>(&'a self, record: &'a StatusRecord) -> Option<&'a str> {
        if let Some(app) = record.app.as_deref() {
            return Some(app);
        }
        let mut id = record.id.as_str();
        while !id.is_empty() {
            id = id.rsplit_once('/').map_or("", |(parent, _)| parent);
            if let Some(app) = self
                .records
                .iter()
                .find(|other| other.id == id)
                .and_then(|other| other.app.as_deref())
            {
                return Some(app);
            }
        }
        None
    }
}

fn within(id: &str, ancestor: &str) -> bool {
    id == ancestor
        || id
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

struct Report {
    id: String,
    /// None for `clear`.
    record: Option<StatusRecord>,
}

fn parse_report(body: &[u8]) -> Option<Report> {
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;
    for pair in body.split(|byte| *byte == b':') {
        let Some(equals) = pair.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = pair[..equals].trim_ascii();
        let value = pair[equals + 1..].trim_ascii();
        if key.len() > MAX_KEY_BYTES {
            return None;
        }
        // Skipping a bad id would point the report at the program's own record.
        if key == b"id" && !value.iter().all(|byte| in_value_set(*byte)) {
            return None;
        }
        if key.is_empty()
            || !key.iter().all(u8::is_ascii_lowercase)
            || !value.iter().all(|byte| in_value_set(*byte))
        {
            continue;
        }
        match key {
            b"state" => state = Some(value),
            b"id" => id = Some(value),
            b"kind" => kind = Some(value),
            b"progress" => progress = Some(value),
            b"app" => app = Some(value),
            b"title" => title = Some(value),
            b"msg" => msg = Some(value),
            _ => {}
        }
    }
    let state = match state? {
        b"idle" => Some(ReportedState::Idle),
        b"working" => Some(ReportedState::Working),
        b"blocked" => Some(ReportedState::Blocked),
        b"done" => Some(ReportedState::Done),
        b"error" => Some(ReportedState::Error),
        b"clear" => None,
        _ => return None,
    };
    let id = match id {
        Some(id) => valid_id(id)?,
        None => String::new(),
    };
    let title = decode_text(title, MAX_TITLE_ENCODED, MAX_TITLE_DECODED)?;
    let msg = decode_text(msg, MAX_MSG_ENCODED, MAX_MSG_DECODED)?;
    let Some(state) = state else {
        return Some(Report { id, record: None });
    };
    let blocked_on = (state == ReportedState::Blocked)
        .then(|| match kind? {
            b"permission" => Some(BlockedOn::Permission),
            b"question" => Some(BlockedOn::Question),
            b"auth" => Some(BlockedOn::Auth),
            _ => None,
        })
        .flatten();
    let progress = matches!(state, ReportedState::Working | ReportedState::Blocked)
        .then(|| {
            std::str::from_utf8(progress?)
                .ok()?
                .parse::<u8>()
                .ok()
                .filter(|percent| *percent <= 100)
        })
        .flatten();
    let app = app
        .filter(|app| {
            (1..=MAX_APP_BYTES).contains(&app.len()) && app.iter().all(|byte| in_name_set(*byte))
        })
        .map(|app| String::from_utf8_lossy(app).into_owned());
    Some(Report {
        record: Some(StatusRecord {
            id: id.clone(),
            state,
            blocked_on,
            progress,
            app,
            title,
            msg,
            touched: 0,
        }),
        id,
    })
}

fn in_value_set(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_.,+/=-".contains(&byte)
}

fn in_name_set(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte)
}

fn valid_id(id: &[u8]) -> Option<String> {
    if id.len() > MAX_ID_BYTES {
        return None;
    }
    let mut depth = 0;
    for segment in id.split(|byte| *byte == b'/') {
        depth += 1;
        if segment.is_empty()
            || segment.len() > MAX_ID_SEGMENT_BYTES
            || depth > MAX_ID_DEPTH
            || !segment.iter().all(|byte| in_name_set(*byte))
        {
            return None;
        }
    }
    Some(String::from_utf8_lossy(id).into_owned())
}

/// Outer None discards the whole report; inner None means the key was absent.
fn decode_text(
    encoded: Option<&[u8]>,
    max_encoded: usize,
    max_decoded: usize,
) -> Option<Option<String>> {
    let Some(encoded) = encoded else {
        return Some(None);
    };
    if encoded.len() > max_encoded {
        return None;
    }
    let decoded = BASE64.decode(encoded).ok()?;
    if decoded.len() > max_decoded {
        return None;
    }
    let text = String::from_utf8(decoded).ok()?;
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(Some(text))
}
