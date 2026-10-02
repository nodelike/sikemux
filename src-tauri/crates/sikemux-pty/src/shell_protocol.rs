use std::time::Duration;

/// Stable frontend event for opt-in local shell metadata. The terminal byte
/// stream remains untouched; this is a second, typed signal derived from it.
pub const PTY_SHELL_METADATA_EVENT: &str = "pty_shell_metadata";
const MAX_SHELL_OSC_BYTES: usize = 8 * 1024;
const MAX_SHELL_PATH_BYTES: usize = 4 * 1024;
const MAX_SHELL_EXIT_CODE_BYTES: usize = 11;
const SHELL_EVENT_MIN_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellPhase {
    #[default]
    Unknown,
    Prompt,
    Input,
    Running,
    Finished,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellMetadataSnapshot {
    pub revision: u64,
    pub cwd: Option<String>,
    pub phase: ShellPhase,
    pub last_exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellBoundary {
    Cwd,
    PromptStart,
    CommandStart,
    CommandExecuted,
    CommandFinished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellProtocolUpdate {
    boundary: ShellBoundary,
    metadata: ShellMetadataSnapshot,
}

#[derive(Default)]
struct ShellProtocolBatch {
    latest: Option<ShellProtocolUpdate>,
    coalesced: usize,
    dropped: usize,
}

impl ShellProtocolBatch {
    fn push(&mut self, update: ShellProtocolUpdate) {
        if self.latest.replace(update).is_some() {
            self.coalesced = self.coalesced.saturating_add(1);
        }
    }
}

#[derive(Default)]
struct ShellEventCoalescer {
    last_emitted_ms: Option<u64>,
    pending: Option<ShellProtocolUpdate>,
}

struct ShellEventDecision {
    ready: Option<ShellProtocolUpdate>,
    replaced_pending: bool,
}

impl ShellEventCoalescer {
    fn submit(&mut self, now_ms: u64, update: ShellProtocolUpdate) -> ShellEventDecision {
        let replaced_pending = self.pending.replace(update).is_some();
        ShellEventDecision {
            ready: self.take_due(now_ms),
            replaced_pending,
        }
    }

    fn take_due(&mut self, now_ms: u64) -> Option<ShellProtocolUpdate> {
        let due = self.last_emitted_ms.is_none_or(|last| {
            now_ms.saturating_sub(last) >= SHELL_EVENT_MIN_INTERVAL.as_millis() as u64
        });
        if !due {
            return None;
        }
        let ready = self.pending.take()?;
        self.last_emitted_ms = Some(now_ms);
        Some(ready)
    }
}

#[derive(Default)]
pub struct ShellProtocolOutput {
    pub ready: Option<ShellProtocolUpdate>,
    pub coalesced: usize,
    pub dropped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyShellMetadataEvent<Id = u32> {
    pub pty_id: Id,
    pub revision: u64,
    pub boundary: ShellBoundary,
    pub cwd: Option<String>,
    pub phase: ShellPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl<Id> PtyShellMetadataEvent<Id> {
    pub fn from_update(pty_id: Id, update: ShellProtocolUpdate) -> Self {
        let exit_code = (update.boundary == ShellBoundary::CommandFinished)
            .then_some(update.metadata.last_exit_code)
            .flatten();
        Self {
            pty_id,
            revision: update.metadata.revision,
            boundary: update.boundary,
            cwd: update.metadata.cwd,
            phase: update.metadata.phase,
            exit_code,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ShellScanState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
}

enum ShellSignal {
    Cwd(String),
    Boundary(ShellBoundary, ShellPhase, Option<i32>),
}

pub struct ShellProtocolParser {
    scan_state: ShellScanState,
    osc: Vec<u8>,
    osc_overflowed: bool,
    metadata: ShellMetadataSnapshot,
    events: ShellEventCoalescer,
}

impl Default for ShellProtocolParser {
    fn default() -> Self {
        Self {
            scan_state: ShellScanState::Ground,
            osc: Vec::with_capacity(256),
            osc_overflowed: false,
            metadata: ShellMetadataSnapshot {
                revision: 0,
                cwd: None,
                phase: ShellPhase::Unknown,
                last_exit_code: None,
            },
            events: ShellEventCoalescer::default(),
        }
    }
}

impl ShellProtocolParser {
    pub fn snapshot(&self) -> ShellMetadataSnapshot {
        self.metadata.clone()
    }

    /// Carries on from a snapshot another parser took.
    pub fn restored(metadata: ShellMetadataSnapshot) -> Self {
        Self {
            metadata,
            ..Self::default()
        }
    }

    fn process(&mut self, bytes: &[u8]) -> ShellProtocolBatch {
        let mut batch = ShellProtocolBatch::default();
        for &byte in bytes {
            match self.scan_state {
                ShellScanState::Ground => {
                    if byte == b'\x1b' {
                        self.scan_state = ShellScanState::Escape;
                    }
                }
                ShellScanState::Escape => {
                    if byte == b']' {
                        self.osc.clear();
                        self.osc_overflowed = false;
                        self.scan_state = ShellScanState::Osc;
                    } else if byte != b'\x1b' {
                        self.scan_state = ShellScanState::Ground;
                    }
                }
                ShellScanState::Osc => match byte {
                    b'\x07' => self.finish_osc(&mut batch),
                    b'\x1b' => self.scan_state = ShellScanState::OscEscape,
                    _ => self.push_osc_byte(byte),
                },
                ShellScanState::OscEscape => match byte {
                    b'\\' | b'\x07' => self.finish_osc(&mut batch),
                    b'\x1b' => {
                        self.push_osc_byte(b'\x1b');
                    }
                    _ => {
                        self.push_osc_byte(b'\x1b');
                        self.push_osc_byte(byte);
                        self.scan_state = ShellScanState::Osc;
                    }
                },
            }
        }
        batch
    }

    pub fn process_for_events(&mut self, bytes: &[u8], now_ms: u64) -> ShellProtocolOutput {
        let mut batch = self.process(bytes);
        let Some(latest) = batch.latest.take() else {
            return ShellProtocolOutput {
                ready: self.events.take_due(now_ms),
                coalesced: batch.coalesced,
                dropped: batch.dropped,
            };
        };
        let decision = self.events.submit(now_ms, latest);
        ShellProtocolOutput {
            ready: decision.ready,
            coalesced: batch
                .coalesced
                .saturating_add(usize::from(decision.replaced_pending)),
            dropped: batch.dropped,
        }
    }

    pub fn take_due_event(&mut self, now_ms: u64) -> Option<ShellProtocolUpdate> {
        self.events.take_due(now_ms)
    }

    fn push_osc_byte(&mut self, byte: u8) {
        if self.osc.len() < MAX_SHELL_OSC_BYTES {
            self.osc.push(byte);
        } else {
            self.osc_overflowed = true;
        }
    }

    fn finish_osc(&mut self, batch: &mut ShellProtocolBatch) {
        if self.osc_overflowed {
            batch.dropped = batch.dropped.saturating_add(1);
        } else if let Some(signal) = parse_shell_signal(&self.osc) {
            self.apply_signal(signal, batch);
        }
        self.osc.clear();
        self.osc_overflowed = false;
        self.scan_state = ShellScanState::Ground;
    }

    fn apply_signal(&mut self, signal: ShellSignal, batch: &mut ShellProtocolBatch) {
        let boundary = match signal {
            ShellSignal::Cwd(cwd) => {
                if self.metadata.cwd.as_deref() == Some(cwd.as_str()) {
                    return;
                }
                self.metadata.cwd = Some(cwd);
                ShellBoundary::Cwd
            }
            ShellSignal::Boundary(boundary, phase, exit_code) => {
                self.metadata.phase = phase;
                if boundary == ShellBoundary::CommandFinished {
                    self.metadata.last_exit_code = exit_code;
                }
                boundary
            }
        };
        self.metadata.revision = self.metadata.revision.saturating_add(1);
        let update = ShellProtocolUpdate {
            boundary,
            metadata: self.metadata.clone(),
        };
        batch.push(update);
    }
}

fn parse_shell_signal(payload: &[u8]) -> Option<ShellSignal> {
    if let Some(uri) = payload.strip_prefix(b"7;") {
        return parse_shell_cwd(uri).map(ShellSignal::Cwd);
    }
    let payload = payload.strip_prefix(b"133;")?;
    let mut fields = payload.split(|byte| *byte == b';');
    let marker = fields.next()?;
    match marker {
        b"A" => Some(ShellSignal::Boundary(
            ShellBoundary::PromptStart,
            ShellPhase::Prompt,
            None,
        )),
        b"B" => Some(ShellSignal::Boundary(
            ShellBoundary::CommandStart,
            ShellPhase::Input,
            None,
        )),
        b"C" => Some(ShellSignal::Boundary(
            ShellBoundary::CommandExecuted,
            ShellPhase::Running,
            None,
        )),
        b"D" => {
            let exit_code = fields.next().and_then(parse_shell_exit_code);
            Some(ShellSignal::Boundary(
                ShellBoundary::CommandFinished,
                ShellPhase::Finished,
                exit_code,
            ))
        }
        _ => None,
    }
}

fn parse_shell_exit_code(value: &[u8]) -> Option<i32> {
    if value.is_empty() || value.len() > MAX_SHELL_EXIT_CODE_BYTES {
        return None;
    }
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_shell_cwd(uri: &[u8]) -> Option<String> {
    if uri.is_empty()
        || uri.len() > MAX_SHELL_OSC_BYTES
        || uri.iter().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    let uri = std::str::from_utf8(uri).ok()?;
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if url
        .host_str()
        .is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
    {
        return None;
    }
    let path = url.to_file_path().ok()?;
    if !path.is_absolute() {
        return None;
    }
    let raw = path.to_string_lossy();
    if raw.is_empty() || raw.len() > MAX_SHELL_PATH_BYTES || raw.chars().any(char::is_control) {
        return None;
    }
    Some(raw.into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_shell_cwd, PtyShellMetadataEvent, ShellBoundary, ShellPhase, ShellProtocolParser,
        MAX_SHELL_OSC_BYTES, MAX_SHELL_PATH_BYTES, SHELL_EVENT_MIN_INTERVAL,
    };
    use crate::screen::{semantic_parser_with_shell, PARSER_SCROLLBACK};
    use std::path::PathBuf;

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_protocol_parses_chunked_cwd_and_command_boundaries() {
        let mut parser = ShellProtocolParser::default();
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\repo root")
        } else {
            PathBuf::from("/tmp/repo root")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let cwd_signal = format!("plain\x1b]7;{cwd_uri}");
        let split = cwd_signal.len() - 2;
        let first = parser.process(&cwd_signal.as_bytes()[..split]);
        assert!(first.latest.is_none());

        let mut second_chunk = cwd_signal.as_bytes()[split..].to_vec();
        second_chunk.extend_from_slice(
            b"\x1b\\\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x1b\\\x1b]133;D;7\x07",
        );
        let second = parser.process(&second_chunk);
        assert_eq!(second.dropped, 0);
        assert_eq!(second.coalesced, 4);
        let latest = second.latest.expect("latest coalesced update");
        assert_eq!(latest.boundary, ShellBoundary::CommandFinished);
        assert_eq!(latest.metadata.revision, 5);
        assert_eq!(latest.metadata.cwd.as_deref(), cwd.to_str());
        assert_eq!(latest.metadata.phase, ShellPhase::Finished);
        assert_eq!(latest.metadata.last_exit_code, Some(7));

        for (signal, expected) in [
            (b"\x1b]133;A\x07".as_slice(), ShellBoundary::PromptStart),
            (b"\x1b]133;B\x07".as_slice(), ShellBoundary::CommandStart),
            (b"\x1b]133;C\x07".as_slice(), ShellBoundary::CommandExecuted),
            (
                b"\x1b]133;D;0\x07".as_slice(),
                ShellBoundary::CommandFinished,
            ),
        ] {
            assert_eq!(
                ShellProtocolParser::default()
                    .process(signal)
                    .latest
                    .expect("boundary update")
                    .boundary,
                expected
            );
        }
    }

    #[test]
    fn shell_protocol_rejects_remote_and_bounds_untrusted_state() {
        assert_eq!(parse_shell_cwd(b"https://localhost/tmp"), None);
        assert_eq!(parse_shell_cwd(b"file://remote-host/tmp"), None);

        assert_eq!(parse_shell_cwd(b"file:///tmp/a%0Ab"), None);

        let oversized_path = format!("file:///{}", "a".repeat(MAX_SHELL_PATH_BYTES + 1));
        assert_eq!(parse_shell_cwd(oversized_path.as_bytes()), None);

        let mut parser = ShellProtocolParser::default();
        let mut oversized_osc = b"\x1b]7;file:///".to_vec();
        oversized_osc.extend(std::iter::repeat_n(b'a', MAX_SHELL_OSC_BYTES + 1));
        let partial = parser.process(&oversized_osc);
        assert!(partial.latest.is_none());
        assert!(parser.osc.len() <= MAX_SHELL_OSC_BYTES);
        let recovered = parser.process(b"\x07\x1b]133;A\x07");
        assert_eq!(recovered.dropped, 1);
        assert_eq!(
            recovered.latest.expect("recovered update").boundary,
            ShellBoundary::PromptStart
        );
    }

    #[test]
    fn shell_protocol_coalesces_hostile_batches_to_latest_headless_state() {
        let mut parser = ShellProtocolParser::default();
        let signal_count = 1_000usize;
        let signals = b"\x1b]133;A\x07".repeat(signal_count);
        let batch = parser.process(&signals);
        assert!(batch.latest.is_some());
        assert_eq!(batch.coalesced, signal_count - 1);
        assert_eq!(batch.dropped, 0);
        assert_eq!(parser.snapshot().revision, signal_count as u64);
        assert_eq!(parser.snapshot().phase, ShellPhase::Prompt);
    }

    #[test]
    fn shell_event_gate_rate_limits_and_flushes_one_bounded_pending_update() {
        let mut parser = ShellProtocolParser::default();
        let first = parser.process_for_events(b"\x1b]133;A\x07", 1_000);
        assert_eq!(
            first.ready.expect("first update is immediate").boundary,
            ShellBoundary::PromptStart
        );

        let second = parser.process_for_events(b"\x1b]133;B\x07", 1_001);
        assert!(second.ready.is_none());
        assert_eq!(second.coalesced, 0);
        let third = parser.process_for_events(b"\x1b]133;C\x07", 1_050);
        assert!(third.ready.is_none());
        assert_eq!(third.coalesced, 1, "newest update replaces one pending");
        assert!(parser.take_due_event(1_099).is_none());
        assert_eq!(
            parser
                .take_due_event(1_000 + SHELL_EVENT_MIN_INTERVAL.as_millis() as u64)
                .expect("latest pending update becomes due")
                .boundary,
            ShellBoundary::CommandExecuted
        );
        assert!(parser.events.pending.is_none());
    }

    #[test]
    fn hostile_osc_stream_cannot_exceed_the_per_pty_event_rate() {
        let mut parser = ShellProtocolParser::default();
        let mut emitted = 0usize;
        for now in 0..1_000u64 {
            let output = parser.process_for_events(b"\x1b]133;A\x07", now);
            emitted += usize::from(output.ready.is_some());
        }
        let maximum = 1_000usize / SHELL_EVENT_MIN_INTERVAL.as_millis() as usize;
        assert_eq!(emitted, maximum);
        assert!(parser.events.pending.is_some());
    }

    #[test]
    fn shell_protocol_side_parse_preserves_visible_terminal_output() {
        let mut parser = semantic_parser_with_shell(24, 80, PARSER_SCROLLBACK, true);
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\project")
        } else {
            PathBuf::from("/tmp/project")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let output = format!("before\x1b]7;{cwd_uri}\x07after");
        let split = output.len() / 2;
        for chunk in [&output.as_bytes()[..split], &output.as_bytes()[split..]] {
            let batch = parser
                .callbacks_mut()
                .shell
                .as_mut()
                .expect("enabled shell parser")
                .process(chunk);
            parser.process(chunk);
            assert_eq!(batch.dropped, 0);
        }
        assert_eq!(parser.screen().contents().trim(), "beforeafter");
        assert_eq!(
            parser
                .callbacks()
                .shell
                .as_ref()
                .expect("enabled shell parser")
                .snapshot()
                .cwd
                .as_deref(),
            cwd.to_str()
        );
    }

    #[test]
    fn shell_metadata_event_is_typed_bounded_frontend_payload() {
        let mut parser = ShellProtocolParser::default();
        let cwd = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\project")
        } else {
            PathBuf::from("/tmp/project")
        };
        let cwd_uri = url::Url::from_file_path(&cwd)
            .expect("native absolute path becomes a file URL")
            .to_string();
        let update = parser
            .process(format!("\x1b]7;{cwd_uri}\x07").as_bytes())
            .latest
            .expect("cwd update");
        let value = serde_json::to_value(PtyShellMetadataEvent::from_update(42, update))
            .expect("serialize shell metadata event");
        assert_eq!(value["ptyId"], 42);
        assert_eq!(value["revision"], 1);
        assert_eq!(value["boundary"], "cwd");
        assert_eq!(value["cwd"], cwd.to_string_lossy().as_ref());
        assert_eq!(value["phase"], "unknown");
        assert!(value.get("exitCode").is_none());
    }
}
