//! A sample grid drawn offscreen, then terminals flooding, timed.
//!
//! ```sh
//! cargo run --release --example grid -- out.png
//! ```
//!
//! It saves a picture of one terminal showing colours, styles, wide characters,
//! box drawing, a hovered link and text an input method is composing. Then one
//! terminal and then four side by side scroll a fresh screen of coloured output
//! every frame, and it prints how long frames took to draw: render, layout,
//! prepaint and paint, everything but handing the frame to the GPU.

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, font, point, px, rgb, size, AppContext, Bounds, Context, Entity,
    HeadlessAppContext, Hsla, IntoElement, ParentElement, Render, Styled, Window, WindowBounds,
    WindowOptions,
};
use gpui_terminal_grid::{
    Cursor, CursorShape, Grid, Highlight, Layout, Look, Marked, Metrics, Span, Underline,
};

const COLUMNS: u16 = 120;
const ROWS: u16 = 50;
const FRAMES: usize = 600;

struct Screen {
    rows: Vec<Vec<(u16, String, Look, bool)>>,
    metrics: Option<Metrics>,
}

impl Grid for Screen {
    fn size(&self) -> (u16, u16) {
        (COLUMNS, ROWS)
    }

    fn row(&self, row: u16, each: &mut dyn FnMut(Span<'_>)) {
        for (column, text, look, wide) in &self.rows[usize::from(row)] {
            each(Span {
                column: *column,
                text,
                wide: *wide,
                cluster: false,
                look: *look,
            });
        }
    }

    fn cursor(&self) -> Option<Cursor> {
        let color: Hsla = rgb(0xa277ff).into();
        Some(Cursor {
            row: ROWS - 1,
            column: 2,
            shape: CursorShape::Block,
            color,
            text: rgb(0x0f0f13).into(),
        })
    }

    fn highlights(&self, each: &mut dyn FnMut(Highlight)) {
        each(Highlight {
            row: 3,
            columns: 4..30,
            color: rgb(0x3d375e).into(),
            underline: false,
        });
        each(Highlight {
            row: 2,
            columns: 7..11,
            color: rgb(0xedecee).into(),
            underline: true,
        });
    }

    fn marked(&self) -> Option<Marked<'_>> {
        Some(Marked {
            row: ROWS - 1,
            column: 2,
            text: "にほんご",
            fg: rgb(0xedecee).into(),
            bg: rgb(0x0f0f13).into(),
        })
    }
}

const PALETTE: [u32; 8] = [
    0xedecee, 0xff6767, 0x54ffbe, 0xffca85, 0xa277ff, 0xff6ac1, 0x61ffca, 0x8b8898,
];

/// The same output every run, so runs compare.
struct Words(u64);

impl Words {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % below.max(1)
    }

    fn line(&mut self) -> Vec<(u16, String, Look, bool)> {
        let mut spans = Vec::new();
        let mut column = 0;
        while column < COLUMNS - 12 {
            let length = 3 + self.next(9) as u16;
            let text: String = (0..length)
                .map(|_| (b'a' + self.next(26) as u8) as char)
                .collect();
            let look = Look {
                bold: self.next(5) == 0,
                ..Look::plain(rgb(PALETTE[self.next(8)]).into())
            };
            spans.push((column, text, look, false));
            column += length + 1;
        }
        spans
    }
}

fn sample() -> Vec<Vec<(u16, String, Look, bool)>> {
    let plain = |color: u32| Look::plain(rgb(color).into());
    let mut rows: Vec<Vec<(u16, String, Look, bool)>> = vec![Vec::new(); usize::from(ROWS)];
    rows[0] = vec![(
        0,
        "╭─ gpui-terminal-grid ────────────────────╮".into(),
        plain(0x8b8898),
        false,
    )];
    rows[1] = vec![
        (0, "│".into(), plain(0x8b8898), false),
        (
            2,
            "bold".into(),
            Look {
                bold: true,
                ..plain(0xedecee)
            },
            false,
        ),
        (
            7,
            "italic".into(),
            Look {
                italic: true,
                ..plain(0xedecee)
            },
            false,
        ),
        (
            14,
            "under".into(),
            Look {
                underline: Some(Underline::Single),
                ..plain(0x54ffbe)
            },
            false,
        ),
        (
            20,
            "curly".into(),
            Look {
                underline: Some(Underline::Curly),
                underline_color: Some(rgb(0xff6767).into()),
                ..plain(0xedecee)
            },
            false,
        ),
        (
            26,
            "strike".into(),
            Look {
                strike: true,
                ..plain(0xffca85)
            },
            false,
        ),
        (
            33,
            "bg".into(),
            Look {
                bg: Some(rgb(0xa277ff).into()),
                ..plain(0x0f0f13)
            },
            false,
        ),
        (42, "│".into(), plain(0x8b8898), false),
    ];
    rows[2] = vec![
        (0, "│".into(), plain(0x8b8898), false),
        (2, "漢字".into(), plain(0xff6ac1), true),
        (7, "wide → ok ✓".into(), plain(0xedecee), false),
        (42, "│".into(), plain(0x8b8898), false),
    ];
    rows[3] = vec![
        (0, "│".into(), plain(0x8b8898), false),
        (
            2,
            "selected text over a highlight".into(),
            plain(0xedecee),
            false,
        ),
        (42, "│".into(), plain(0x8b8898), false),
    ];
    rows[4] = vec![(
        0,
        "╰─────────────────────────────────────────╯".into(),
        plain(0x8b8898),
        false,
    )];
    rows[6] = vec![(
        0,
        "▁▂▃▄▅▆▇█ ░▒▓ ┏━┳━┓ ╔═╦═╗ ".into(),
        plain(0x61ffca),
        false,
    )];
    rows[usize::from(ROWS) - 1] = vec![(0, "$ ".into(), plain(0xa277ff), false)];
    rows
}

struct Panes(Vec<Entity<Screen>>);

impl Render for Panes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_wrap()
            .bg(rgb(0x0f0f13))
            .children(self.0.iter().map(|screen| {
                let screen = screen.clone();
                div().w(px(1100.)).h(px(860.)).child(
                    canvas(
                        move |bounds, window, cx| {
                            let metrics = screen.update(cx, |screen, _| {
                                screen
                                    .metrics
                                    .get_or_insert_with(|| {
                                        Metrics::new(font("Menlo"), px(13.), 1.32, window)
                                    })
                                    .clone()
                            });
                            Layout::new(screen.read(cx), bounds.origin, &metrics, window)
                        },
                        |_, layout, window, cx| layout.paint(window, cx),
                    )
                    .size_full(),
                )
            }))
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "grid.png".into());
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    let mut words = Words(7);
    for count in [1usize, 4] {
        let screens: Vec<Entity<Screen>> = (0..count)
            .map(|_| {
                cx.update(|cx| {
                    cx.new(|_| Screen {
                        rows: sample(),
                        metrics: None,
                    })
                })
            })
            .collect();
        let panes = screens.clone();
        let window = cx
            .update(|cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: point(px(0.), px(0.)),
                            size: if count == 1 {
                                size(px(1100.), px(860.))
                            } else {
                                size(px(2200.), px(1720.))
                            },
                        })),
                        focus: false,
                        show: false,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Panes(panes)),
                )
            })
            .expect("window opens");
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .ok();
        if count == 1 {
            if let Ok(image) = cx.capture_screenshot(window.into()) {
                image.save(&out).ok();
            }
        }
        let mut frames = Vec::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            for screen in &screens {
                let fresh: Vec<_> = (0..8).map(|_| words.line()).collect();
                screen.update(&mut cx, |screen, cx| {
                    screen.rows.drain(..8);
                    screen.rows.extend(fresh);
                    cx.notify();
                });
            }
            let began = Instant::now();
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .ok();
            frames.push(began.elapsed().as_secs_f64() * 1e3);
        }
        frames.sort_by(f64::total_cmp);
        let at = |q: f64| frames[((frames.len() - 1) as f64 * q).round() as usize];
        println!(
            "{count} terminal(s) of {COLUMNS}x{ROWS}, 8 fresh lines each a frame: draw p50 {:.2} ms, p95 {:.2}, p99 {:.2}, max {:.2}",
            at(0.5),
            at(0.95),
            at(0.99),
            at(1.)
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .ok();
    }
}
