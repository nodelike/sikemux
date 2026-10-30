//! The terminal itself: the core reads a program's output into this, and
//! clients get its cells. Built on alacritty's emulator.

mod counting;
mod frame;
mod history;
pub mod input;
mod replay;
mod text;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, Rgb};
use sikemux_wire::grid::{SpanStyle, TerminalColors};

use history::History;

pub use frame::View;

/// Lines a terminal keeps above its screen.
pub const HISTORY_LINES: usize = 10_000;
/// Lines kept while nobody watches a terminal that has gone quiet.
pub const IDLE_HISTORY_LINES: usize = 1_000;

#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// What the emulator asks of the core while it reads output.
#[derive(Clone, Default)]
pub struct Events(Arc<Mutex<Vec<Event>>>);

impl Events {
    fn take(&self) -> Vec<Event> {
        std::mem::take(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

impl EventListener for Events {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::Title(_) | Event::ResetTitle | Event::PtyWrite(_) | Event::ColorRequest(..)
        ) {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        }
    }
}

/// What reading a chunk of output produced besides the new cells.
#[derive(Default)]
pub struct Processed {
    /// Answers to the program's queries, to write back to it.
    pub replies: Vec<u8>,
    /// The program set its title, or `Some("")` when it reset it.
    pub title: Option<String>,
}

pub struct Terminal {
    term: Term<Events>,
    events: Events,
    parser: Processor,
    history: History,
    /// Lines ever pushed into the history; the number of the top row.
    pushed: u64,
    epoch: u32,
    /// Changes whenever anything a client draws may have changed.
    version: u64,
    palette_version: u32,
    frames: frame::Hashes,
    text: String,
}

/// The emulator keeps only a screen's worth of history, which is moved into
/// the terminal's own after every call that pushes lines.
fn config(rows: usize) -> Config {
    Config {
        scrolling_history: rows,
        default_cursor_style: CursorStyle {
            shape: CursorShape::Block,
            blinking: true,
        },
        ..Config::default()
    }
}

impl Terminal {
    pub fn new(rows: u16, cols: u16) -> Self {
        let size = Size {
            cols: usize::from(cols.max(1)),
            rows: usize::from(rows.max(1)),
        };
        let events = Events::default();
        Self {
            term: Term::new(config(size.rows), &size, events.clone()),
            events,
            parser: Processor::new(),
            history: History::new(HISTORY_LINES),
            pushed: 0,
            epoch: 0,
            version: 1,
            palette_version: 0,
            frames: frame::Hashes::default(),
            text: String::new(),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.term.screen_lines() as u16, self.term.columns() as u16)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn alternate_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn process(&mut self, bytes: &[u8], colors: &TerminalColors) -> Processed {
        let mut sink = counting::Counting::new(&mut self.term, &mut self.history);
        self.parser.advance(&mut sink, bytes);
        let outcome = sink.finish();
        self.absorb(outcome);
        self.take_events(colors)
    }

    /// When a synchronized update the program started must end at the
    /// latest, if one is open.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Shows a synchronized update whose program never ended it.
    pub fn end_sync(&mut self, colors: &TerminalColors) -> Processed {
        let mut sink = counting::Counting::new(&mut self.term, &mut self.history);
        self.parser.stop_sync(&mut sink);
        let outcome = sink.finish();
        self.absorb(outcome);
        self.take_events(colors)
    }

    fn absorb(&mut self, outcome: counting::Outcome) {
        self.pushed += outcome.pushed;
        if outcome.cleared {
            self.history.clear();
            self.epoch = self.epoch.wrapping_add(1);
        }
        if outcome.palette {
            self.palette_version = self.palette_version.wrapping_add(1);
        }
        self.version += 1;
    }

    fn take_events(&mut self, colors: &TerminalColors) -> Processed {
        let mut processed = Processed::default();
        for event in self.events.take() {
            match event {
                Event::Title(title) => processed.title = Some(title),
                Event::ResetTitle => processed.title = Some(String::new()),
                Event::PtyWrite(text) => processed.replies.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, format) => {
                    let [r, g, b] = match self.term.colors()[index] {
                        Some(set) => [set.r, set.g, set.b],
                        None => colors.color(index),
                    };
                    processed
                        .replies
                        .extend_from_slice(format(Rgb { r, g, b }).as_bytes());
                }
                _ => {}
            }
        }
        processed
    }

    /// Resizes the screen, rewrapping the main screen's lines and the
    /// history to the new width. Line numbers start over, since rewrapping
    /// changes them.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = Size {
            cols: usize::from(cols.max(1)),
            rows: usize::from(rows.max(1)),
        };
        let old_cols = self.term.columns();
        if (size.rows, size.cols) == (self.term.screen_lines(), old_cols) {
            return;
        }
        self.term.resize(size);
        self.term.set_options(config(size.rows));
        let _ = self.events.take();
        if !self.alternate_screen() {
            self.pushed += counting::move_history(
                &mut self.term,
                &mut self.history,
                &mut self.text,
                &mut Vec::new(),
            );
        }
        if size.cols != old_cols {
            let before = self.history.len() as u64;
            let after = self.history.rewrap(size.cols) as u64;
            self.pushed = (self.pushed + after).saturating_sub(before).max(after);
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.version += 1;
    }

    /// Moves the cursor's line to the top and drops everything above it, as
    /// a person clearing the scrollback expects. The program is not told.
    pub fn clear(&mut self, colors: &TerminalColors) {
        let row = self.term.grid().cursor.point.line.0.max(0);
        let sequence = if row > 0 {
            format!("\x1b[{row}S\x1b[3J\x1b[{row}A")
        } else {
            "\x1b[3J".to_owned()
        };
        let _ = self.process(sequence.as_bytes(), colors);
    }

    /// Keeps at most `lines` of history, dropping the oldest beyond it.
    pub fn set_history_limit(&mut self, lines: usize) {
        self.history.set_limit(lines);
        self.version += 1;
    }

    pub fn history_limit(&self) -> usize {
        self.history.limit()
    }

    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// The number of the first history line still kept.
    fn first_line(&self) -> u64 {
        self.pushed - self.history.len() as u64
    }

    /// The number of the bottom row of the screen.
    fn last_line(&self) -> u64 {
        self.pushed + self.term.screen_lines() as u64 - 1
    }

    /// Calls `each` with the spans of line `line`, from the history or the
    /// screen, and answers whether it goes on in the next line, or nothing
    /// when the line is not kept.
    fn walk_line(
        &self,
        line: u64,
        each: impl FnMut(&SpanStyle<'_>, bool, bool, &str),
    ) -> Option<bool> {
        if line < self.first_line() || line > self.last_line() {
            return None;
        }
        if line < self.pushed {
            let stored = self.history.line((line - self.first_line()) as usize)?;
            return Some(history::walk(stored, each));
        }
        let cols = self.term.columns();
        let row = &self.term.grid()[Line((line - self.pushed) as i32)];
        frame::walk_row(row, cols, &mut String::new(), each);
        Some(frame::wrapped(row, cols))
    }
}
