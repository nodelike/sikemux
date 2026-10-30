//! Turns raw terminal output into the lines a person would see once it settled:
//! escape sequences are dropped, and carriage returns and cursor moves are
//! replayed so a redrawn progress line appears once, in its final state.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub end: u64,
}

enum State {
    Ground,
    Escape,
    Csi(String),
    ControlString,
    ControlStringEscape,
    Charset,
}

const MAX_COLUMN: usize = 10_000;
const MAX_ROWS_OPENED_BY_CURSOR: usize = 10_000;
const MAX_PADDING_CELLS: usize = 1 << 20;

struct Screen {
    rows: Vec<Vec<char>>,
    ends: Vec<Option<u64>>,
    row: usize,
    col: usize,
    rows_left_for_cursor: usize,
    padding_left: usize,
}

impl Screen {
    fn new() -> Self {
        Self {
            rows: vec![Vec::new()],
            ends: vec![None],
            row: 0,
            col: 0,
            rows_left_for_cursor: MAX_ROWS_OPENED_BY_CURSOR,
            padding_left: MAX_PADDING_CELLS,
        }
    }

    fn set_col(&mut self, col: usize) {
        self.col = col.min(MAX_COLUMN);
    }

    fn move_down(&mut self, count: usize) {
        let last = self.rows.len() - 1;
        let reachable = last + self.rows_left_for_cursor;
        let target = self.row.saturating_add(count).min(reachable);
        self.rows_left_for_cursor -= target.saturating_sub(last);
        self.go_to_row(target);
    }

    fn go_to_row(&mut self, row: usize) {
        self.row = row;
        while self.rows.len() <= row {
            self.rows.push(Vec::new());
            self.ends.push(None);
        }
    }

    fn put(&mut self, character: char) {
        let line = &mut self.rows[self.row];
        if line.len() < self.col {
            let padding = (self.col - line.len()).min(self.padding_left);
            self.padding_left -= padding;
            line.resize(line.len() + padding, ' ');
            self.col = line.len();
        }
        if self.col < line.len() {
            line[self.col] = character;
        } else {
            line.push(character);
        }
        self.set_col(self.col + 1);
    }

    fn newline(&mut self, at: u64) {
        let end = &mut self.ends[self.row];
        *end = Some(end.map_or(at, |previous| previous.max(at)));
        self.go_to_row(self.row + 1);
        self.col = 0;
    }

    fn apply(&mut self, parameters: &str, command: char) {
        if parameters.starts_with(['?', '>', '<', '=']) {
            return;
        }
        let numbers: Vec<usize> = parameters
            .split(';')
            .map(|value| {
                value
                    .parse()
                    .map_or(0, |number: usize| number.min(MAX_COLUMN))
            })
            .collect();
        let first = numbers.first().copied().unwrap_or(0);
        let count = first.max(1);
        match command {
            'A' => self.row = self.row.saturating_sub(count),
            'B' | 'e' => self.move_down(count),
            'C' | 'a' => self.set_col(self.col + count),
            'D' => self.col = self.col.saturating_sub(count),
            'E' => {
                self.move_down(count);
                self.col = 0;
            }
            'F' => {
                self.row = self.row.saturating_sub(count);
                self.col = 0;
            }
            'G' | '`' => self.set_col(count - 1),
            'H' | 'f' => self.set_col(numbers.get(1).copied().unwrap_or(0).max(1) - 1),
            'K' => {
                let line = &mut self.rows[self.row];
                match first {
                    0 => line.truncate(self.col),
                    1 => {
                        for cell in line.iter_mut().take(self.col + 1) {
                            *cell = ' ';
                        }
                    }
                    _ => line.clear(),
                }
            }
            'J' if first == 0 => {
                self.rows[self.row].truncate(self.col);
                self.rows.truncate(self.row + 1);
                self.ends.truncate(self.row + 1);
            }
            _ => {}
        }
    }
}

pub fn render(bytes: &[u8], first_offset: u64) -> Vec<Line> {
    let mut screen = Screen::new();
    let mut state = State::Ground;
    let mut offset = first_offset;
    for chunk in bytes.utf8_chunks() {
        for character in chunk.valid().chars() {
            offset += character.len_utf8() as u64;
            state = match state {
                State::Ground => match character {
                    '\x1b' => State::Escape,
                    '\r' => {
                        screen.col = 0;
                        State::Ground
                    }
                    '\n' => {
                        screen.newline(offset);
                        State::Ground
                    }
                    '\x08' => {
                        screen.col = screen.col.saturating_sub(1);
                        State::Ground
                    }
                    '\t' => {
                        screen.set_col((screen.col / 8 + 1) * 8);
                        State::Ground
                    }
                    other if other.is_control() => State::Ground,
                    other => {
                        screen.put(other);
                        State::Ground
                    }
                },
                State::Escape => match character {
                    '[' => State::Csi(String::new()),
                    ']' | 'P' | '_' | '^' | 'X' => State::ControlString,
                    '(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%' => State::Charset,
                    'E' => {
                        screen.newline(offset);
                        State::Ground
                    }
                    'M' => {
                        screen.row = screen.row.saturating_sub(1);
                        State::Ground
                    }
                    _ => State::Ground,
                },
                State::Csi(mut parameters) => {
                    if ('\x40'..='\x7e').contains(&character) {
                        screen.apply(&parameters, character);
                        State::Ground
                    } else if parameters.len() < 64 {
                        parameters.push(character);
                        State::Csi(parameters)
                    } else {
                        State::Ground
                    }
                }
                State::ControlString => match character {
                    '\x07' => State::Ground,
                    '\x1b' => State::ControlStringEscape,
                    _ => State::ControlString,
                },
                State::ControlStringEscape | State::Charset => State::Ground,
            };
        }
        offset += chunk.invalid().len() as u64;
    }
    let total = first_offset + bytes.len() as u64;
    if screen.rows.len() > 1
        && screen.ends.last() == Some(&None)
        && screen.rows.last().is_some_and(Vec::is_empty)
    {
        screen.rows.pop();
        screen.ends.pop();
    }
    let mut reached = first_offset;
    screen
        .rows
        .into_iter()
        .zip(screen.ends)
        .map(|(row, end)| {
            reached = reached.max(end.unwrap_or(total));
            Line {
                text: row.into_iter().collect::<String>().trim_end().to_string(),
                end: reached,
            }
        })
        .collect()
}

const LONGEST_REPEATED_BLOCK: usize = 32;

pub fn collapse(lines: Vec<Line>) -> Vec<Line> {
    let mut collapsed = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        let mut best: Option<(usize, usize)> = None;
        for size in 1..=LONGEST_REPEATED_BLOCK.min((lines.len() - index) / 2) {
            let block = &lines[index..index + size];
            if block.iter().all(|line| line.text.is_empty()) {
                continue;
            }
            let mut copies = 1;
            while index + (copies + 1) * size <= lines.len()
                && lines[index + copies * size..index + (copies + 1) * size]
                    .iter()
                    .zip(block)
                    .all(|(line, first)| line.text == first.text)
            {
                copies += 1;
            }
            let hidden = size * (copies - 1);
            if hidden >= 2 && best.is_none_or(|(_, most)| hidden > most) {
                best = Some((size, hidden));
            }
        }
        let Some((size, hidden)) = best else {
            collapsed.push(lines[index].clone());
            index += 1;
            continue;
        };
        collapsed.extend_from_slice(&lines[index..index + size]);
        let more = hidden / size;
        let times = if more == 1 { "time" } else { "times" };
        let text = if size == 1 {
            format!("[repeated {more} more {times}]")
        } else {
            format!("[previous {size} lines repeated {more} more {times}]")
        };
        index += size + hidden;
        collapsed.push(Line {
            text,
            end: lines[index - 1].end,
        });
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn strips_colours_and_title_sequences() {
        let lines = render(
            b"\x1b]0;title\x07\x1b[1;31merror\x1b[0m: bad\r\n\x1b(Bok\n",
            0,
        );
        assert_eq!(texts(&lines), ["error: bad", "ok"]);
    }

    #[test]
    fn carriage_returns_keep_only_the_final_redraw() {
        let lines = render(b"progress 10%\rprogress 50%\rprogress 100%\ndone\n", 0);
        assert_eq!(texts(&lines), ["progress 100%", "done"]);
    }

    #[test]
    fn cursor_up_redraws_replace_earlier_lines() {
        let output = b"build a 1/3\nbuild b 1/3\n\x1b[2A\x1b[0Gbuild a 3/3\x1b[K\nbuild b 3/3\x1b[K\nfinished\n";
        let lines = render(output, 0);
        assert_eq!(texts(&lines), ["build a 3/3", "build b 3/3", "finished"]);
    }

    #[test]
    fn erase_below_drops_rows_that_were_redrawn_away() {
        let lines = render(b"one\ntwo\nthree\n\x1b[2A\x1b[Jtwo again\n", 0);
        assert_eq!(texts(&lines), ["one", "two again"]);
    }

    #[test]
    fn line_ends_are_output_offsets_that_never_go_backwards() {
        let lines = render(b"ab\ncd\nef", 10);
        assert_eq!(
            lines.iter().map(|line| line.end).collect::<Vec<_>>(),
            [13, 16, 18]
        );
        let redrawn = render(b"a\nb\n\x1b[2Ax\n", 0);
        assert!(redrawn.windows(2).all(|pair| pair[0].end <= pair[1].end));
    }

    #[test]
    fn repeated_lines_and_blocks_collapse_to_one_copy() {
        let lines = render(b"start\nsame\nsame\nsame\nsame\nend\n", 0);
        assert_eq!(
            texts(&collapse(lines)),
            ["start", "same", "[repeated 3 more times]", "end"]
        );
        let block = b"lock\n----\n(1 row)\nlock\n----\n(1 row)\nlock\n----\n(1 row)\nready\n";
        assert_eq!(
            texts(&collapse(render(block, 0))),
            [
                "lock",
                "----",
                "(1 row)",
                "[previous 3 lines repeated 2 more times]",
                "ready"
            ]
        );
        let pair = render(b"a\na\nb\n\n\n\n", 0);
        assert_eq!(texts(&collapse(pair)), ["a", "a", "b", "", "", ""]);
    }

    #[test]
    fn huge_cursor_moves_stay_bounded() {
        let right = render(b"\x1b[18446744073709551615Cx\n", 0);
        assert_eq!(right[0].text.trim_start(), "x");
        assert!(right[0].text.chars().count() <= MAX_COLUMN + 1);

        let down = render(b"a\x1b[50000000Bb\n", 0);
        assert!(down.len() <= MAX_ROWS_OPENED_BY_CURSOR + 2);
        assert_eq!(down.last().unwrap().text.trim(), "b");

        let repeated = b"\x1b[9999Bx".repeat(1000);
        assert!(render(&repeated, 0).len() <= MAX_ROWS_OPENED_BY_CURSOR + 2);

        let wide = b"\x1b[9999Cx\n".repeat(1000);
        let cells: usize = render(&wide, 0)
            .iter()
            .map(|line| line.text.chars().count())
            .sum();
        assert!(cells <= MAX_PADDING_CELLS + 1000);

        render(&b"\x1b[99999999999999999999999C".repeat(100), 0);
        render(&b"\t".repeat(100_000), 0);
    }
}
