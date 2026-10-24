//! Hands the parsed output to the emulator, and moves every line that scrolls
//! into its history into the terminal's own compact history at once.
//! Clients number history lines by the lines pushed, so the count has to be
//! exact.
//!
//! The emulator keeps a screen's worth of history, more than one call can
//! push, and is emptied after each call: its history lines never change
//! once pushed, so each is stored the moment it leaves the screen.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
    ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};

use super::frame::store_row;
use super::history::History;
use super::Events;

#[derive(Default)]
pub(super) struct Outcome {
    pub pushed: u64,
    /// The history was cleared or replaced, so earlier line numbers are void.
    pub cleared: bool,
    pub palette: bool,
}

pub(super) struct Counting<'a> {
    term: &'a mut Term<Events>,
    history: &'a mut History,
    text: String,
    scratch: Vec<u8>,
    outcome: Outcome,
}

impl<'a> Counting<'a> {
    pub(super) fn new(term: &'a mut Term<Events>, history: &'a mut History) -> Self {
        Self {
            term,
            history,
            text: String::new(),
            scratch: Vec::new(),
            outcome: Outcome::default(),
        }
    }

    pub(super) fn finish(self) -> Outcome {
        self.outcome
    }

    #[inline]
    fn counted(&mut self, act: impl FnOnce(&mut Term<Events>)) {
        act(self.term);
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.outcome.pushed +=
            move_history(self.term, self.history, &mut self.text, &mut self.scratch);
    }
}

/// Stores the emulator's history lines, oldest first, and empties its
/// history. Answers how many there were.
pub(super) fn move_history(
    term: &mut Term<Events>,
    history: &mut History,
    text: &mut String,
    scratch: &mut Vec<u8>,
) -> u64 {
    let pushed = term.grid().history_size();
    if pushed == 0 {
        return 0;
    }
    let cols = term.columns();
    let grid = term.grid();
    for age in (1..=pushed).rev() {
        history.push(store_row(&grid[Line(-(age as i32))], cols, text, scratch));
    }
    let room = term.screen_lines();
    let grid = term.grid_mut();
    grid.update_history(0);
    grid.update_history(room);
    pushed as u64
}

macro_rules! pass {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            #[inline]
            fn $name(&mut self, $($arg: $ty),*) {
                self.term.$name($($arg),*)
            }
        )*
    };
}

macro_rules! count {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            #[inline]
            fn $name(&mut self, $($arg: $ty),*) {
                self.counted(|term| term.$name($($arg),*))
            }
        )*
    };
}

impl Handler for Counting<'_> {
    count! {
        input(c: char);
        linefeed();
        newline();
        scroll_up(lines: usize);
        delete_lines(lines: usize);
        set_private_mode(mode: PrivateMode);
        unset_private_mode(mode: PrivateMode);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        let history = matches!(mode, ClearMode::Saved);
        self.counted(|term| term.clear_screen(mode));
        if history {
            self.outcome.cleared = true;
        }
    }

    fn reset_state(&mut self) {
        self.term.reset_state();
        self.outcome.cleared = true;
        self.outcome.palette = true;
    }

    fn set_color(&mut self, index: usize, color: Rgb) {
        self.term.set_color(index, color);
        self.outcome.palette = true;
    }

    fn reset_color(&mut self, index: usize) {
        self.term.reset_color(index);
        self.outcome.palette = true;
    }

    pass! {
        set_title(title: Option<String>);
        set_cursor_style(style: Option<CursorStyle>);
        set_cursor_shape(shape: CursorShape);
        goto(line: i32, col: usize);
        goto_line(line: i32);
        goto_col(col: usize);
        insert_blank(count: usize);
        move_up(lines: usize);
        move_down(lines: usize);
        identify_terminal(intermediate: Option<char>);
        device_status(arg: usize);
        move_forward(col: usize);
        move_backward(col: usize);
        move_down_and_cr(row: usize);
        move_up_and_cr(row: usize);
        put_tab(count: u16);
        backspace();
        carriage_return();
        bell();
        substitute();
        set_horizontal_tabstop();
        scroll_down(lines: usize);
        insert_blank_lines(lines: usize);
        erase_chars(count: usize);
        delete_chars(count: usize);
        move_backward_tabs(count: u16);
        move_forward_tabs(count: u16);
        save_cursor_position();
        restore_cursor_position();
        clear_line(mode: LineClearMode);
        clear_tabs(mode: TabulationClearMode);
        set_tabs(interval: u16);
        reverse_index();
        terminal_attribute(attr: Attr);
        set_mode(mode: Mode);
        unset_mode(mode: Mode);
        report_mode(mode: Mode);
        report_private_mode(mode: PrivateMode);
        set_scrolling_region(top: usize, bottom: Option<usize>);
        set_keypad_application_mode();
        unset_keypad_application_mode();
        set_active_charset(index: CharsetIndex);
        configure_charset(index: CharsetIndex, charset: StandardCharset);
        clipboard_store(clipboard: u8, base64: &[u8]);
        clipboard_load(clipboard: u8, terminator: &str);
        decaln();
        push_title();
        pop_title();
        text_area_size_pixels();
        text_area_size_chars();
        set_hyperlink(link: Option<Hyperlink>);
        report_keyboard_mode();
        push_keyboard_mode(mode: KeyboardModes);
        pop_keyboard_modes(count: u16);
        set_keyboard_mode(mode: KeyboardModes, behavior: KeyboardModesApplyBehavior);
        set_modify_other_keys(mode: ModifyOtherKeys);
        report_modify_other_keys();
        set_scp(path: ScpCharPath, update: ScpUpdateMode);
    }

    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str) {
        self.term.dynamic_color_sequence(prefix, index, terminator)
    }
}
