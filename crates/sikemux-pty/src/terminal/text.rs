use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use regex::RegexBuilder;
use sikemux_wire::terminal::{FindOptions, FindResult, GridPoint};

use super::Terminal;

const MAX_MATCHES: usize = 1_000;

impl Terminal {
    /// The cursor's row and column on the screen.
    pub fn cursor(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        (point.line.0.max(0) as u16, point.column.0 as u16)
    }

    /// The lowest screen row with anything written in it.
    pub fn last_written_row(&self) -> Option<u16> {
        let grid = self.term.grid();
        let cols = self.term.columns();
        (0..self.term.screen_lines()).rev().find_map(|line| {
            let row = &grid[Line(line as i32)];
            (0..cols)
                .any(|column| !matches!(row[Column(column)].c, ' ' | '\t'))
                .then_some(line as u16)
        })
    }

    /// The text between two cells, both included, lines that wrapped joined.
    pub fn text(&self, from: GridPoint, to: GridPoint) -> String {
        let (from, to) = if to < from { (to, from) } else { (from, to) };
        let first = from.line.max(self.first_line());
        let last = to.line.min(self.last_line());
        let cols = self.term.columns() as u16;
        let mut text = String::new();
        let mut scratch = String::new();
        for line in first..=last {
            let start = if line == from.line { from.col } else { 0 };
            let end = if line == to.line { to.col } else { u16::MAX };
            let mut col = 0u16;
            let Some(wrapped) = self.walk_line(line, &mut scratch, |_, wide, cluster, span| {
                let width = if wide { 2 } else { 1 };
                if cluster {
                    if col >= start && col <= end {
                        text.push_str(span);
                    }
                    col = col.saturating_add(width);
                } else if col >= start && end == u16::MAX {
                    text.push_str(span);
                    let count = if span.is_ascii() {
                        span.len()
                    } else {
                        span.chars().count()
                    };
                    col = col.saturating_add((count as u16).saturating_mul(width));
                } else {
                    for character in span.chars() {
                        if col >= start && col <= end {
                            text.push(character);
                        }
                        col = col.saturating_add(width);
                    }
                }
            }) else {
                continue;
            };
            if line == last {
                break;
            }
            if wrapped {
                if end >= cols {
                    text.extend(std::iter::repeat_n(
                        ' ',
                        usize::from(cols.saturating_sub(col)),
                    ));
                }
            } else {
                text.truncate(text.trim_end_matches(' ').len());
                text.push('\n');
            }
        }
        text.truncate(text.trim_end().len());
        text
    }

    /// Everything the terminal still holds, history and screen.
    pub fn all_text(&self) -> String {
        self.text(
            GridPoint {
                line: self.first_line(),
                col: 0,
            },
            GridPoint {
                line: self.last_line(),
                col: u16::MAX,
            },
        )
    }

    /// The screen's text the way agent detection reads it: rows that wrapped
    /// joined, blanks after a row's last character and empty rows at the
    /// bottom left out.
    pub fn contents(&self) -> String {
        let grid = self.term.grid();
        let cols = self.term.columns();
        let mut contents = String::new();
        for line in 0..self.term.screen_lines() {
            let row = &grid[Line(line as i32)];
            let mut pending = 0;
            for column in 0..cols {
                let cell = &row[Column(column)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                if cell.c == ' ' || cell.c == '\t' {
                    pending += 1;
                    continue;
                }
                contents.extend(std::iter::repeat_n(' ', pending));
                pending = 0;
                contents.push(cell.c);
                contents.extend(cell.zerowidth().into_iter().flatten());
            }
            if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                contents.push('\n');
            }
        }
        while contents.ends_with('\n') {
            contents.pop();
        }
        contents
    }

    /// The screen row by row as a person reads it, with dim text such as an
    /// empty prompt's placeholder left out.
    pub fn read_rows(&self) -> Vec<String> {
        let grid = self.term.grid();
        let cols = self.term.columns();
        (0..self.term.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                let mut text = String::new();
                for column in 0..cols {
                    let cell = &row[Column(column)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    if cell.flags.contains(Flags::DIM) {
                        text.push(' ');
                    } else {
                        text.push(cell.c);
                        text.extend(cell.zerowidth().into_iter().flatten());
                    }
                }
                text.truncate(text.trim_end().len());
                text
            })
            .collect()
    }

    pub fn find(
        &self,
        query: &str,
        options: &FindOptions,
        from: Option<GridPoint>,
        backwards: bool,
        inclusive: bool,
    ) -> Result<FindResult, String> {
        let mut result = FindResult {
            epoch: self.epoch,
            ..FindResult::default()
        };
        if query.is_empty() {
            return Ok(result);
        }
        let mut pattern = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        if options.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .build()
            .map_err(|error| error.to_string())?;
        let cols = self.term.columns() as u16;
        let mut text = String::new();
        let mut scratch = String::new();
        let mut places: Vec<(usize, GridPoint, u16)> = Vec::new();
        let mut line = self.first_line();
        'lines: while line <= self.last_line() {
            text.clear();
            places.clear();
            loop {
                let mut col = 0u16;
                let Some(wrapped) = self.walk_line(line, &mut scratch, |_, wide, cluster, span| {
                    let width = if wide { 2 } else { 1 };
                    if cluster {
                        places.push((text.len(), GridPoint { line, col }, width));
                        text.push_str(span);
                        col = col.saturating_add(width);
                    } else {
                        let base = text.len();
                        text.push_str(span);
                        for (offset, _) in span.char_indices() {
                            places.push((base + offset, GridPoint { line, col }, width));
                            col = col.saturating_add(width);
                        }
                    }
                }) else {
                    line += 1;
                    break;
                };
                line += 1;
                if !wrapped || line > self.last_line() {
                    break;
                }
                while col < cols {
                    places.push((
                        text.len(),
                        GridPoint {
                            line: line - 1,
                            col,
                        },
                        1,
                    ));
                    text.push(' ');
                    col += 1;
                }
            }
            for found in regex.find_iter(&text) {
                if found.is_empty() {
                    continue;
                }
                if result.matches.len() == MAX_MATCHES {
                    result.more = true;
                    break 'lines;
                }
                let at = |offset: usize| match places.binary_search_by_key(&offset, |place| place.0)
                {
                    Ok(index) => index,
                    Err(index) => index.saturating_sub(1),
                };
                let (Some(&(_, start, _)), Some(&(_, end, width))) = (
                    places.get(at(found.start())),
                    places.get(at(found.end() - 1)),
                ) else {
                    continue;
                };
                result.matches.push((
                    start,
                    GridPoint {
                        line: end.line,
                        col: end.col + width - 1,
                    },
                ));
            }
        }
        if result.matches.is_empty() {
            return Ok(result);
        }
        let last = result.matches.len() - 1;
        result.current = Some(match (from, backwards) {
            (None, false) => 0,
            (None, true) => last,
            (Some(from), false) => result
                .matches
                .iter()
                .position(|(start, _)| *start > from || (inclusive && *start == from))
                .unwrap_or(0),
            (Some(from), true) => result
                .matches
                .iter()
                .rposition(|(start, _)| *start < from || (inclusive && *start == from))
                .unwrap_or(last),
        });
        Ok(result)
    }
}
