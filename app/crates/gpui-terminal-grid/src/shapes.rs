//! Box drawing, block and powerline characters, drawn as shapes filling the
//! cell, so borders and rules join up whatever the font's glyphs and the row
//! height are.

use gpui::{point, px, size, Bounds, Hsla, PathBuilder, Pixels, Point};

use crate::Mark;

const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Line weights toward the top, right, bottom and left edges of the cell.
fn arms(character: char) -> Option<[u8; 4]> {
    const L: u8 = LIGHT;
    const H: u8 = HEAVY;
    const D: u8 = DOUBLE;
    Some(match character {
        '─' => [0, L, 0, L],
        '━' => [0, H, 0, H],
        '│' => [L, 0, L, 0],
        '┃' => [H, 0, H, 0],
        '┌' => [0, L, L, 0],
        '┍' => [0, H, L, 0],
        '┎' => [0, L, H, 0],
        '┏' => [0, H, H, 0],
        '┐' => [0, 0, L, L],
        '┑' => [0, 0, L, H],
        '┒' => [0, 0, H, L],
        '┓' => [0, 0, H, H],
        '└' => [L, L, 0, 0],
        '┕' => [L, H, 0, 0],
        '┖' => [H, L, 0, 0],
        '┗' => [H, H, 0, 0],
        '┘' => [L, 0, 0, L],
        '┙' => [L, 0, 0, H],
        '┚' => [H, 0, 0, L],
        '┛' => [H, 0, 0, H],
        '├' => [L, L, L, 0],
        '┝' => [L, H, L, 0],
        '┠' => [H, L, H, 0],
        '┣' => [H, H, H, 0],
        '┤' => [L, 0, L, L],
        '┥' => [L, 0, L, H],
        '┨' => [H, 0, H, L],
        '┫' => [H, 0, H, H],
        '┬' => [0, L, L, L],
        '┯' => [0, H, L, H],
        '┰' => [0, L, H, L],
        '┳' => [0, H, H, H],
        '┴' => [L, L, 0, L],
        '┷' => [L, H, 0, H],
        '┸' => [H, L, 0, L],
        '┻' => [H, H, 0, H],
        '┼' => [L, L, L, L],
        '┿' => [L, H, L, H],
        '╂' => [H, L, H, L],
        '╋' => [H, H, H, H],
        '═' => [0, D, 0, D],
        '║' => [D, 0, D, 0],
        '╔' => [0, D, D, 0],
        '╗' => [0, 0, D, D],
        '╚' => [D, D, 0, 0],
        '╝' => [D, 0, 0, D],
        '╠' => [D, D, D, 0],
        '╣' => [D, 0, D, D],
        '╦' => [0, D, D, D],
        '╩' => [D, D, 0, D],
        '╬' => [D, D, D, D],
        '╴' => [0, 0, 0, L],
        '╵' => [L, 0, 0, 0],
        '╶' => [0, L, 0, 0],
        '╷' => [0, 0, L, 0],
        '╸' => [0, 0, 0, H],
        '╹' => [H, 0, 0, 0],
        '╺' => [0, H, 0, 0],
        '╻' => [0, 0, H, 0],
        _ => return None,
    })
}

/// Draws `character` filling `cell`, answering false for one left to the font.
pub(crate) fn draw(
    character: char,
    cell: Bounds<Pixels>,
    stroke: Pixels,
    color: Hsla,
    marks: &mut Vec<Mark>,
) -> bool {
    if !matches!(character, '\u{2500}'..='\u{259f}' | '\u{e0b0}'..='\u{e0b3}') {
        return false;
    }
    if let Some(arms) = arms(character) {
        draw_arms(arms, cell, stroke, color, marks);
        return true;
    }
    let edges = match character {
        '╭' => Some([1, 2]),
        '╮' => Some([2, 3]),
        '╯' => Some([0, 3]),
        '╰' => Some([0, 1]),
        _ => None,
    };
    if let Some(edges) = edges {
        return draw_arc(edges, cell, stroke, color, marks);
    }
    draw_block(character, cell, stroke, color, marks)
}

fn rect(x: Pixels, y: Pixels, width: Pixels, height: Pixels, color: Hsla) -> Mark {
    Mark::Fill(Bounds::new(point(x, y), size(width, height)), color)
}

/// Where lines through the cell's middle start, on whole pixels so they join the next cell's.
fn middle(cell: Bounds<Pixels>, stroke: Pixels) -> Point<Pixels> {
    point(
        cell.origin.x + ((cell.size.width - stroke) / 2.).floor(),
        cell.origin.y + ((cell.size.height - stroke) / 2.).floor(),
    )
}

fn draw_arms(
    [top, right, bottom, left]: [u8; 4],
    cell: Bounds<Pixels>,
    stroke: Pixels,
    color: Hsla,
    marks: &mut Vec<Mark>,
) {
    let Bounds {
        origin: Point { x, y },
        size: cell_size,
    } = cell;
    let Point { x: cx, y: cy } = middle(cell, stroke);
    let (width, height) = (cell_size.width, cell_size.height);
    if [top, right, bottom, left].contains(&DOUBLE) {
        let horizontal = left == DOUBLE || right == DOUBLE;
        let vertical = top == DOUBLE || bottom == DOUBLE;
        for offset in [-stroke, stroke] {
            if horizontal {
                let from = if left != 0 { x } else { cx };
                let to = if right != 0 { x + width } else { cx + stroke };
                marks.push(rect(from, cy + offset, to - from, stroke, color));
            }
            if vertical {
                let from = if top != 0 { y } else { cy };
                let to = if bottom != 0 { y + height } else { cy + stroke };
                marks.push(rect(cx + offset, from, stroke, to - from, color));
            }
        }
        return;
    }
    let thick = |weight: u8| if weight == HEAVY { stroke * 2. } else { stroke };
    if left != 0 {
        let t = thick(left);
        marks.push(rect(x, cy - (t - stroke) / 2., cx - x + stroke, t, color));
    }
    if right != 0 {
        let t = thick(right);
        marks.push(rect(cx, cy - (t - stroke) / 2., x + width - cx, t, color));
    }
    if top != 0 {
        let t = thick(top);
        marks.push(rect(cx - (t - stroke) / 2., y, t, cy - y + stroke, color));
    }
    if bottom != 0 {
        let t = thick(bottom);
        marks.push(rect(cx - (t - stroke) / 2., cy, t, y + height - cy, color));
    }
}

/// A rounded corner joining two edges, numbered top, right, bottom, left.
fn draw_arc(
    edges: [usize; 2],
    cell: Bounds<Pixels>,
    stroke: Pixels,
    color: Hsla,
    marks: &mut Vec<Mark>,
) -> bool {
    let Bounds {
        origin: Point { x, y },
        size: cell_size,
    } = cell;
    let center = middle(cell, stroke) + point(stroke / 2., stroke / 2.);
    let ends = [
        point(center.x, y),
        point(x + cell_size.width, center.y),
        point(center.x, y + cell_size.height),
        point(x, center.y),
    ];
    let (from, to) = (ends[edges[0]], ends[edges[1]]);
    let radius = f32::from(cell_size.width.min(cell_size.height)) / 2.;
    let toward = |end: Point<Pixels>| {
        let (dx, dy) = (f32::from(end.x - center.x), f32::from(end.y - center.y));
        let share = radius.min(dx.hypot(dy)) / dx.hypot(dy).max(f32::EPSILON);
        point(center.x + px(dx * share), center.y + px(dy * share))
    };
    let mut path = PathBuilder::stroke(stroke);
    path.move_to(from);
    path.line_to(toward(from));
    path.curve_to(toward(to), center);
    path.line_to(to);
    match path.build() {
        Ok(path) => {
            marks.push(Mark::Path(path, color));
            true
        }
        Err(_) => false,
    }
}

fn draw_block(
    character: char,
    cell: Bounds<Pixels>,
    stroke: Pixels,
    color: Hsla,
    marks: &mut Vec<Mark>,
) -> bool {
    let Bounds {
        origin: Point { x, y },
        size: cell_size,
    } = cell;
    let (width, height) = (cell_size.width, cell_size.height);
    let code = character as u32;
    match code {
        0x2588 => marks.push(rect(x, y, width, height, color)),
        0x2581..=0x2587 => {
            let part = (height * ((code - 0x2580) as f32 / 8.)).round();
            marks.push(rect(x, y + height - part, width, part, color));
        }
        0x2580 => marks.push(rect(x, y, width, (height / 2.).round(), color)),
        0x2589..=0x258f => marks.push(rect(
            x,
            y,
            (width * ((0x2590 - code) as f32 / 8.)).round(),
            height,
            color,
        )),
        0x2590 => {
            let half = (width / 2.).round();
            marks.push(rect(x + half, y, width - half, height, color));
        }
        0x2594 => marks.push(rect(x, y, width, (height / 8.).round().max(stroke), color)),
        0x2595 => {
            let part = (width / 8.).round().max(stroke);
            marks.push(rect(x + width - part, y, part, height, color));
        }
        0x2591..=0x2593 => marks.push(rect(
            x,
            y,
            width,
            height,
            Hsla {
                a: color.a * (code - 0x2590) as f32 / 4.,
                ..color
            },
        )),
        0xe0b0..=0xe0b3 => {
            let points = if code <= 0xe0b1 {
                [
                    point(x, y),
                    point(x + width, y + height / 2.),
                    point(x, y + height),
                ]
            } else {
                [
                    point(x + width, y),
                    point(x, y + height / 2.),
                    point(x + width, y + height),
                ]
            };
            let solid = code == 0xe0b0 || code == 0xe0b2;
            let mut path = if solid {
                PathBuilder::fill()
            } else {
                PathBuilder::stroke((width / 8.).round().max(stroke))
            };
            path.add_polygon(&points, solid);
            match path.build() {
                Ok(path) => marks.push(Mark::Path(path, color)),
                Err(_) => return false,
            }
        }
        _ => return false,
    }
    true
}
