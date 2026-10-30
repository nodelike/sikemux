use sikemux_wire::grid::TerminalColors;

use crate::shell_protocol::{ShellMetadataSnapshot, ShellProtocolParser, SshLink};
use crate::terminal::{Processed, Terminal, HISTORY_LINES, IDLE_HISTORY_LINES};

const MAX_TITLE_CHARS: usize = 120;

/// A session's terminal, with the title and shell state read from the same
/// output.
pub struct Screen {
    pub terminal: Terminal,
    pub window_title: String,
    /// Set when the title changed and nobody has taken the change yet.
    pub title_changed: bool,
    /// The folder a shell last reported, which names its terminal.
    titled_cwd: Option<String>,
    pub shell: Option<ShellProtocolParser>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16, shell: bool) -> Self {
        Self {
            terminal: Terminal::new(rows, cols),
            window_title: String::new(),
            title_changed: false,
            titled_cwd: None,
            shell: shell.then(ShellProtocolParser::default),
        }
    }

    /// A screen rebuilt from [`Terminal::replay`] bytes, with the shell
    /// state and title the replay cannot carry.
    pub fn restored(
        rows: u16,
        cols: u16,
        replay: &[u8],
        shell: Option<ShellMetadataSnapshot>,
        ssh: Option<SshLink>,
        window_title: String,
    ) -> Self {
        let mut screen = Self::new(rows, cols, false);
        screen.process(replay, &TerminalColors::default());
        screen.window_title = window_title;
        screen.title_changed = false;
        screen.shell = shell.map(|shell| ShellProtocolParser::restored(shell, ssh));
        screen
    }

    /// Reads output into the terminal. Answers to the program's queries come
    /// back to be written to it.
    pub fn process(&mut self, bytes: &[u8], colors: &TerminalColors) -> Vec<u8> {
        let processed = self.terminal.process(bytes, colors);
        self.absorb(processed)
    }

    pub fn end_sync(&mut self, colors: &TerminalColors) -> Vec<u8> {
        let processed = self.terminal.end_sync(colors);
        self.absorb(processed)
    }

    fn absorb(&mut self, processed: Processed) -> Vec<u8> {
        if let Some(title) = processed.title {
            let title = readable_title(title.as_bytes());
            if title != self.window_title {
                self.window_title = title;
                self.title_changed = true;
            }
        }
        processed.replies
    }

    pub fn take_title_change(&mut self) -> Option<String> {
        std::mem::take(&mut self.title_changed).then(|| self.window_title.clone())
    }

    /// A shell that reports its folder is named after it each time it moves.
    pub fn title_from_cwd(&mut self) {
        let cwd = self.shell.as_ref().and_then(|shell| shell.snapshot().cwd);
        if cwd == self.titled_cwd {
            return;
        }
        self.titled_cwd = cwd;
        let Some(name) = self.titled_cwd.as_deref().map(|cwd| {
            std::path::Path::new(cwd).file_name().map_or_else(
                || cwd.to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        }) else {
            return;
        };
        let title = readable_title(name.as_bytes());
        if title != self.window_title {
            self.window_title = title;
            self.title_changed = true;
        }
    }

    /// Nobody is looking, so keep only the newest history. The alternate
    /// screen has none to drop.
    pub fn compact_for_idle(&mut self) -> bool {
        if self.terminal.alternate_screen() || self.terminal.history_len() <= IDLE_HISTORY_LINES {
            return false;
        }
        self.terminal.set_history_limit(IDLE_HISTORY_LINES);
        true
    }

    /// Lets a compacted terminal keep its full history again.
    pub fn restore_history(&mut self) {
        if self.terminal.history_limit() != HISTORY_LINES {
            self.terminal.set_history_limit(HISTORY_LINES);
        }
    }
}

/// Titles are untrusted child-process output: control characters become
/// spaces, runs of space become one, and a long title ends in an ellipsis.
fn readable_title(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let words: Vec<&str> = text
        .split(|ch: char| ch.is_control() || ch.is_whitespace())
        .filter(|word| !word.is_empty())
        .collect();
    let title = words.join(" ");
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title;
    }
    let kept: String = title.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use sikemux_wire::grid::TerminalColors;

    use super::Screen;

    #[test]
    fn a_restored_screen_shows_the_same_screen_history_and_shell_state() {
        let mut original = Screen::new(5, 20, true);
        let colors = TerminalColors::default();
        for i in 0..12 {
            original.process(format!("row {i:02}\r\n").as_bytes(), &colors);
        }
        original.process(b"\x1b]2;Busy\x07$ ", &colors);
        let shell = crate::shell_protocol::ShellMetadataSnapshot {
            revision: 4,
            cwd: Some("/tmp".into()),
            phase: crate::shell_protocol::ShellPhase::Prompt,
            last_exit_code: Some(2),
        };
        let replay = original.terminal.replay();
        let restored = Screen::restored(5, 20, &replay, Some(shell.clone()), None, "Busy".into());

        assert_eq!(restored.terminal.contents(), original.terminal.contents());
        assert_eq!(restored.terminal.all_text(), original.terminal.all_text());
        assert_eq!(restored.window_title, "Busy");
        assert_eq!(
            restored.shell.as_ref().map(|shell| shell.snapshot()),
            Some(shell)
        );
    }

    #[test]
    fn titles_are_captured_and_sanitized() {
        let mut screen = Screen::new(24, 80, false);
        let colors = TerminalColors::default();
        screen.process(b"\x1b]2;Action\n required\x07", &colors);
        assert_eq!(screen.window_title, "Action required");
        assert_eq!(
            screen.take_title_change().as_deref(),
            Some("Action required")
        );
        assert_eq!(screen.take_title_change(), None);
        screen.process(
            format!("\x1b]2;{}\x07", "🧪".repeat(140)).as_bytes(),
            &colors,
        );
        let title = &screen.window_title;
        assert_eq!(title.chars().count(), 120);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn idle_compaction_keeps_the_newest_history_and_modes() {
        let mut screen = Screen::new(5, 20, false);
        let colors = TerminalColors::default();
        for i in 0..2_100 {
            screen.process(format!("line {i:04}\r\n").as_bytes(), &colors);
        }
        screen.process(b"\x1b[?1h\x1b[?2004h", &colors);

        assert!(screen.compact_for_idle());
        assert!(screen.terminal.bracketed_paste());
        assert!(screen.terminal.history_len() <= super::IDLE_HISTORY_LINES);
        assert!(!screen.terminal.all_text().contains("line 0000"));
        assert!(screen.terminal.contents().contains("line 2099"));
    }
}
