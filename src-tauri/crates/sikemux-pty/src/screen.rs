use std::collections::VecDeque;

use crate::error::{PtyError, PtyResult};
use crate::shell_protocol::{ShellMetadataSnapshot, ShellProtocolParser};

// Scrollback held in the headless vt100 parser. This only has to cover what
// a reattaching xterm replays; anything the user scrolled past before the
// pane was hidden is not worth paying for. A vt100 cell is 32 bytes, so at
// 200 columns 3k rows is roughly 19 MB per PTY — at 10k rows it was 64 MB.
// The parser drops to IDLE_SCROLLBACK when the last subscriber detaches, and
// the sweeper catches anything silent for its idle trim window.
pub const PARSER_SCROLLBACK: usize = 3_000;
pub const IDLE_SCROLLBACK: usize = 1_000;
pub const MAX_ATTACH_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub struct SemanticCallbacks {
    pub window_title: String,
    pub shell: Option<ShellProtocolParser>,
}

impl vt100::Callbacks for SemanticCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        // Titles are untrusted child-process output. Keep only printable text
        // and impose a small scalar limit before retaining it for detection.
        self.window_title = String::from_utf8_lossy(title)
            .chars()
            .filter(|ch| !ch.is_control())
            .take(512)
            .collect();
    }
}

pub type SemanticParser = vt100::Parser<SemanticCallbacks>;

#[cfg(test)]
fn semantic_parser(rows: u16, cols: u16, scrollback: usize) -> SemanticParser {
    semantic_parser_with_shell(rows, cols, scrollback, false)
}

pub fn semantic_parser_with_shell(
    rows: u16,
    cols: u16,
    scrollback: usize,
    enabled: bool,
) -> SemanticParser {
    SemanticParser::new_with_callbacks(
        rows,
        cols,
        scrollback,
        SemanticCallbacks {
            window_title: String::new(),
            shell: enabled.then(ShellProtocolParser::default),
        },
    )
}

/// vt100 clamps the requested offset to the history it actually holds, so
/// asking for the largest possible offset reports the number of history rows.
/// The view is put back at the live viewport before returning.
pub fn screen_scrollback_len(screen: &mut vt100::Screen) -> usize {
    screen.set_scrollback(usize::MAX);
    let rows = screen.scrollback();
    screen.set_scrollback(0);
    rows
}

#[derive(Debug)]
struct BoundedAttachSnapshot {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Produce a replay stream whose returned allocation never exceeds
/// `max_bytes`. When all history cannot fit, only a contiguous newest suffix
/// is retained; the live viewport, terminal modes, and cursor state are never
/// selectively truncated. If that non-negotiable viewport does not fit, the
/// caller gets an error and can leave parser/subscriber state untouched.
fn bounded_attach_snapshot(
    screen: &mut vt100::Screen,
    max_bytes: usize,
) -> PtyResult<BoundedAttachSnapshot> {
    const ALT_SCREEN_PREFIX: &[u8] = b"\x1b[?1049h";
    const HISTORY_ROW_SUFFIX: &[u8] = b"\x1b[0m\r\n";

    let alternate_screen = screen.alternate_screen();
    let prefix = if alternate_screen {
        ALT_SCREEN_PREFIX
    } else {
        &[]
    };
    let viewport = screen.state_formatted();
    let fixed_bytes = prefix
        .len()
        .checked_add(viewport.len())
        .ok_or_else(|| PtyError::Pty("PTY attach snapshot size overflow".into()))?;
    if fixed_bytes > max_bytes {
        return Err(PtyError::Pty(
            "PTY attach viewport exceeds snapshot capacity".into(),
        ));
    }

    let history_rows = if alternate_screen {
        0
    } else {
        screen_scrollback_len(screen)
    };
    if history_rows == 0 {
        let mut bytes = Vec::with_capacity(fixed_bytes);
        bytes.extend_from_slice(prefix);
        bytes.extend_from_slice(&viewport);
        return Ok(BoundedAttachSnapshot {
            bytes,
            truncated: false,
        });
    }

    let (rows, cols) = screen.size();
    let separator_bytes = usize::from(rows.saturating_sub(1))
        .checked_mul(b"\r\n".len())
        .ok_or_else(|| PtyError::Pty("PTY attach snapshot size overflow".into()))?;
    let history_budget = max_bytes.saturating_sub(fixed_bytes.saturating_add(separator_bytes));
    let mut retained = VecDeque::<Vec<u8>>::new();
    let mut retained_bytes = 0usize;
    let mut truncated = history_budget == 0;
    let mut seen_rows = 0usize;

    // Iterate in the same oldest-to-newest page order as the full replay.
    // The deque never retains more than the remaining byte budget; evicting
    // from its front leaves a deterministic newest suffix.
    let page_rows = usize::from(rows).max(1);
    let mut start = 0usize;
    while start < history_rows {
        screen.set_scrollback(history_rows - start);
        let take = (history_rows - start).min(page_rows);
        for mut row in screen.rows_formatted(0, cols).take(take) {
            seen_rows += 1;
            row.extend_from_slice(HISTORY_ROW_SUFFIX);
            if row.len() > history_budget {
                retained.clear();
                retained_bytes = 0;
                truncated = true;
                continue;
            }
            while retained_bytes.saturating_add(row.len()) > history_budget {
                let Some(evicted) = retained.pop_front() else {
                    break;
                };
                retained_bytes = retained_bytes.saturating_sub(evicted.len());
                truncated = true;
            }
            retained_bytes += row.len();
            retained.push_back(row);
        }
        start += take;
    }
    screen.set_scrollback(0);
    truncated |= seen_rows < history_rows || retained.len() < history_rows;

    let include_history = !retained.is_empty();
    let final_separator_bytes = if include_history { separator_bytes } else { 0 };
    let final_capacity = fixed_bytes
        .checked_add(retained_bytes)
        .and_then(|size| size.checked_add(final_separator_bytes))
        .ok_or_else(|| PtyError::Pty("PTY attach snapshot size overflow".into()))?;
    debug_assert!(final_capacity <= max_bytes);
    let mut bytes = Vec::with_capacity(final_capacity);
    bytes.extend_from_slice(prefix);
    for row in retained {
        bytes.extend_from_slice(&row);
    }
    if include_history {
        for _ in 1..rows {
            bytes.extend_from_slice(b"\r\n");
        }
    }
    bytes.extend_from_slice(&viewport);
    debug_assert!(bytes.len() <= max_bytes);
    Ok(BoundedAttachSnapshot { bytes, truncated })
}

fn attach_snapshot(screen: &mut vt100::Screen) -> Vec<u8> {
    let mut snapshot = Vec::new();
    if screen.alternate_screen() {
        // vt100::Screen::state_formatted() restores contents and input
        // modes, but not which screen buffer is active. Re-enter alt
        // screen before replaying alt-buffer contents so xterm's wheel
        // behavior matches the live PTY after a hidden-pane reattach.
        snapshot.extend_from_slice(b"\x1b[?1049h");
    }
    let history_rows = if screen.alternate_screen() {
        0
    } else {
        screen_scrollback_len(screen)
    };
    if history_rows > 0 {
        let (rows, cols) = screen.size();

        // Seed xterm's scrollback cheaply from vt100's formatted semantic
        // history rows only. `state_formatted` below clears/repaints the live
        // viewport with cursor and input modes; replaying the current viewport
        // here would push a duplicate prompt/input line into scrollback on every
        // reattach, which looks like terminal text repeating after tab switches.
        // Reset between rows because each formatted row is generated from
        // default attrs.
        let page_rows = usize::from(rows).max(1);
        let mut start = 0usize;
        while start < history_rows {
            screen.set_scrollback(history_rows - start);
            let take = (history_rows - start).min(page_rows);
            for row in screen.rows_formatted(0, cols).take(take) {
                snapshot.extend(row);
                snapshot.extend_from_slice(b"\x1b[0m\r\n");
            }
            start += take;
        }
        screen.set_scrollback(0);

        // Move the replay cursor far enough that state_formatted's viewport
        // repaint does not overwrite the newest history rows. Without this
        // separator, a fresh parser has a rows-1 hole between the replayed
        // history and the restored live viewport.
        for _ in 1..rows {
            snapshot.extend_from_slice(b"\r\n");
        }
    }
    snapshot.extend(screen.state_formatted());
    snapshot
}

fn reseed_parser_from_snapshot(parser: &mut SemanticParser, snapshot: &[u8], scrollback: usize) {
    let (rows, cols) = parser.screen().size();
    let callbacks = std::mem::take(parser.callbacks_mut());
    let mut fresh = SemanticParser::new_with_callbacks(rows, cols, scrollback, callbacks);
    fresh.process(snapshot);
    *parser = fresh;
}

pub fn reseed_parser(parser: &mut SemanticParser, scrollback: usize) {
    let snapshot = attach_snapshot(parser.screen_mut());
    reseed_parser_from_snapshot(parser, &snapshot, scrollback);
}

pub fn attach_snapshot_with_compaction(
    parser: &mut SemanticParser,
    max_bytes: usize,
) -> PtyResult<Vec<u8>> {
    let snapshot = bounded_attach_snapshot(parser.screen_mut(), max_bytes)?;
    if snapshot.truncated {
        reseed_parser_from_snapshot(parser, &snapshot.bytes, PARSER_SCROLLBACK);
    }
    Ok(snapshot.bytes)
}

/// The bytes that rebuild this screen and its history in a fresh parser,
/// bounded like an attach, without trimming the parser itself.
pub fn replay_snapshot(parser: &mut SemanticParser) -> PtyResult<Vec<u8>> {
    Ok(bounded_attach_snapshot(parser.screen_mut(), MAX_ATTACH_SNAPSHOT_BYTES)?.bytes)
}

/// A parser rebuilt from [`replay_snapshot`] bytes, with the shell state and
/// title the replay cannot carry.
pub fn restored_parser(
    rows: u16,
    cols: u16,
    replay: &[u8],
    shell: Option<ShellMetadataSnapshot>,
    window_title: String,
) -> SemanticParser {
    let mut parser = SemanticParser::new_with_callbacks(
        rows,
        cols,
        PARSER_SCROLLBACK,
        SemanticCallbacks::default(),
    );
    parser.process(replay);
    let callbacks = parser.callbacks_mut();
    callbacks.window_title = window_title;
    callbacks.shell = shell.map(ShellProtocolParser::restored);
    parser
}

pub fn compact_parser_for_idle(parser: &mut SemanticParser) -> bool {
    if parser.screen().alternate_screen() {
        return false;
    }
    reseed_parser(parser, IDLE_SCROLLBACK);
    true
}

#[cfg(test)]
mod tests {
    use super::{
        attach_snapshot, attach_snapshot_with_compaction, compact_parser_for_idle, reseed_parser,
        screen_scrollback_len, semantic_parser,
    };
    use super::{IDLE_SCROLLBACK, MAX_ATTACH_SNAPSHOT_BYTES, PARSER_SCROLLBACK};

    #[test]
    fn a_restored_parser_shows_the_same_screen_history_and_shell_state() {
        let mut original = super::semantic_parser_with_shell(5, 20, PARSER_SCROLLBACK, true);
        for i in 0..12 {
            original.process(format!("row {i:02}\r\n").as_bytes());
        }
        original.process(b"\x1b]2;Busy\x07$ ");
        let shell = crate::shell_protocol::ShellMetadataSnapshot {
            revision: 4,
            cwd: Some("/tmp".into()),
            phase: crate::shell_protocol::ShellPhase::Prompt,
            last_exit_code: Some(2),
        };
        let replay = super::replay_snapshot(&mut original).unwrap();
        let mut restored =
            super::restored_parser(5, 20, &replay, Some(shell.clone()), "Busy".into());

        assert_eq!(restored.screen().contents(), original.screen().contents());
        assert_eq!(
            screen_scrollback_len(restored.screen_mut()),
            screen_scrollback_len(original.screen_mut())
        );
        assert_eq!(restored.callbacks().window_title, "Busy");
        assert_eq!(
            restored
                .callbacks()
                .shell
                .as_ref()
                .map(|shell| shell.snapshot()),
            Some(shell)
        );
    }

    #[test]
    fn semantic_parser_captures_and_sanitizes_osc_title() {
        let mut parser = semantic_parser(24, 80, PARSER_SCROLLBACK);
        parser.process(b"\x1b]2;Action\n required\x07");
        assert_eq!(parser.callbacks().window_title, "Action required");
    }

    #[test]
    fn snapshot_round_trips_visible_state() {
        // Smoke-check the contract pty_attach relies on: a parser whose
        // bytes were processed re-emits an ANSI stream that reproduces the
        // visible state when written back into a fresh parser. The full
        // attach/snapshot path can't be exercised without a real PTY, but
        // the parser invariant is the load-bearing piece.
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"hello world\r\nsecond line\r\n");
        let dump = a.screen().contents_formatted();

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);
        assert_eq!(
            a.screen().contents(),
            b.screen().contents(),
            "snapshot did not round-trip cleanly",
        );
    }

    #[test]
    fn attach_snapshot_restores_input_modes() {
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"\x1b[?2004h\x1b[?1000h\x1b[?1006h");
        let dump = attach_snapshot(a.screen_mut());

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);

        assert!(b.screen().bracketed_paste());
        assert_eq!(
            b.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease,
        );
        assert_eq!(
            b.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr,
        );
    }

    #[test]
    fn attach_snapshot_restores_scrollback() {
        let mut a = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..20 {
            a.process(format!("line {i:02}\r\n").as_bytes());
        }
        let dump = attach_snapshot(a.screen_mut());

        let mut b = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        b.process(&dump);

        assert_eq!(b.screen().contents(), a.screen().contents());

        let screen = b.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(
            screen.scrollback() >= 10,
            "reattach snapshot should seed xterm/vt100 scrollback; got {} rows",
            screen.scrollback()
        );
        assert!(
            screen.contents().contains("line 00"),
            "oldest retained output should be reachable after scrolling"
        );
    }

    #[test]
    fn attach_snapshot_restores_scrollback_attrs() {
        let mut a = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..20 {
            let color = 31 + (i % 6);
            a.process(format!("\x1b[{color}mline {i:02}\x1b[0m\r\n").as_bytes());
        }
        let dump = attach_snapshot(a.screen_mut());

        let mut b = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        b.process(&dump);

        assert_eq!(b.screen().contents(), a.screen().contents());

        let screen = b.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(
            screen.contents().contains("line 00"),
            "oldest retained output should be reachable after scrolling"
        );
        assert_eq!(
            screen
                .cell(0, 0)
                .expect("top-left scrollback cell")
                .fgcolor(),
            vt100::Color::Idx(1),
            "reattach snapshot should preserve attrs for scrolled-out rows"
        );
    }

    #[test]
    fn attach_snapshot_restores_alternate_screen() {
        let mut a = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        a.process(b"normal\r\n\x1b[?1049halt");
        let dump = attach_snapshot(a.screen_mut());

        let mut b = vt100::Parser::new(24, 80, PARSER_SCROLLBACK);
        b.process(&dump);

        assert!(b.screen().alternate_screen());
        assert_eq!(b.screen().contents(), a.screen().contents());
    }

    #[test]
    fn attach_snapshot_has_exact_history_continuity_without_blank_hole() {
        let mut source = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        for i in 0..30 {
            source.process(format!("line {i:02}\r\n").as_bytes());
        }

        let mut restored = vt100::Parser::new(5, 20, PARSER_SCROLLBACK);
        restored.process(&attach_snapshot(source.screen_mut()));

        let source_screen = source.screen_mut();
        source_screen.set_scrollback(5);
        let expected = source_screen.contents();
        source_screen.set_scrollback(0);
        let expected_viewport = source_screen.contents();

        let screen = restored.screen_mut();
        screen.set_scrollback(5);
        let joined = screen.contents();
        assert_eq!(joined, expected, "history-to-viewport boundary has a hole");
        assert!(!joined.lines().any(|line| line.trim().is_empty()));
        screen.set_scrollback(0);
        assert_eq!(screen.contents(), expected_viewport);
    }

    #[test]
    fn attach_snapshot_byte_budget_compacts_only_old_history() {
        let mut parser = semantic_parser(6, 40, PARSER_SCROLLBACK);
        for index in 0..500 {
            parser.process(
                format!(
                    "\x1b[{}mline {index:04} {}\x1b[0m\r\n",
                    31 + index % 7,
                    "x".repeat(32)
                )
                .as_bytes(),
            );
        }
        parser.process(b"\x1b[?2004h");

        let visible_before = parser.screen().contents();
        let history_before = screen_scrollback_len(parser.screen_mut());
        let full_before = attach_snapshot(parser.screen_mut());
        let viewport_bytes = parser.screen().state_formatted().len();
        let budget = viewport_bytes + 512;
        assert!(full_before.len() > budget, "fixture must force compaction");

        let snapshot = attach_snapshot_with_compaction(&mut parser, budget)
            .expect("bounded snapshot retains viewport");
        assert!(snapshot.len() <= budget);
        assert_eq!(parser.screen().contents(), visible_before);
        assert!(parser.screen().bracketed_paste());
        assert!(screen_scrollback_len(parser.screen_mut()) < history_before);

        let mut restored = vt100::Parser::new(6, 40, PARSER_SCROLLBACK);
        restored.process(&snapshot);
        assert_eq!(restored.screen().contents(), visible_before);
        assert!(restored.screen().bracketed_paste());
        assert_eq!(MAX_ATTACH_SNAPSHOT_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn attach_snapshot_viewport_over_budget_fails_without_mutation() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        for index in 0..20 {
            parser.process(format!("line {index:02}\r\n").as_bytes());
        }
        parser.process(b"\x1b[?1000h\x1b[?1006h");
        let before = attach_snapshot(parser.screen_mut());
        let viewport_bytes = parser.screen().state_formatted().len();
        assert!(viewport_bytes > 0);

        assert!(attach_snapshot_with_compaction(&mut parser, viewport_bytes - 1).is_err());
        assert_eq!(attach_snapshot(parser.screen_mut()), before);
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::PressRelease
        );
        assert_eq!(
            parser.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr
        );
    }

    #[test]
    fn idle_compaction_retains_tail_and_modes() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        for i in 0..2_100 {
            parser.process(format!("line {i:04}\r\n").as_bytes());
        }
        parser.process(b"\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h");

        assert!(compact_parser_for_idle(&mut parser));
        assert!(parser.screen().application_cursor());
        assert!(parser.screen().bracketed_paste());
        assert_eq!(
            parser.screen().mouse_protocol_mode(),
            vt100::MouseProtocolMode::ButtonMotion
        );
        assert_eq!(
            parser.screen().mouse_protocol_encoding(),
            vt100::MouseProtocolEncoding::Sgr
        );
        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(screen.scrollback() <= IDLE_SCROLLBACK);
        assert!(!screen.contents().contains("line 0000"));
        screen.set_scrollback(0);
        assert!(screen.contents().contains("line 2099"));
    }

    #[test]
    fn idle_compaction_skips_alternate_screen() {
        let mut parser = semantic_parser(5, 20, PARSER_SCROLLBACK);
        parser.process(b"normal history\r\n\x1b[?1049halt screen");
        let before = attach_snapshot(parser.screen_mut());

        assert!(!compact_parser_for_idle(&mut parser));
        assert_eq!(attach_snapshot(parser.screen_mut()), before);
        assert!(parser.screen().alternate_screen());
    }

    #[test]
    fn reseed_restores_full_future_scrollback_capacity() {
        let mut parser = semantic_parser(5, 20, IDLE_SCROLLBACK);
        for i in 0..1_000 {
            parser.process(format!("old {i:04}\r\n").as_bytes());
        }
        reseed_parser(&mut parser, PARSER_SCROLLBACK);
        for i in 0..1_500 {
            parser.process(format!("new {i:04}\r\n").as_bytes());
        }

        let screen = parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        assert!(screen.scrollback() > IDLE_SCROLLBACK);
        assert!(screen.contents().contains("old 0000"));
    }
}
