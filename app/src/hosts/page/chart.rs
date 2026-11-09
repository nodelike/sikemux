//! The page's meters and history charts, painted straight onto a canvas as a few paths each,
//! kept between draws while what they show stays the same.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, point, px, relative, AnyElement, Bounds, FontWeight, Hsla, IntoElement,
    ParentElement, Path, PathBuilder, Pixels, Point, Styled, Window,
};

use crate::theme::{Tokens, TEXT_3XS};
use crate::ui;

/// How many built paths are kept for drawing again unchanged, before they are all let go.
const KEPT_PATHS: usize = 512;

thread_local! {
    /// Built paths by where they are drawn and what they draw, so a redraw that moves nothing,
    /// such as a hover, paints them without building them again.
    static PATHS: RefCell<HashMap<u64, Path<Pixels>>> = RefCell::new(HashMap::new());
}

/// Paints the path `key` names, building it only when no path of that key is kept.
fn paint_kept(
    window: &mut Window,
    key: impl Hash,
    tint: Hsla,
    build: impl FnOnce() -> Option<Path<Pixels>>,
) {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    let key = hasher.finish();
    let path = PATHS.with(|paths| {
        let mut paths = paths.borrow_mut();
        if let Some(path) = paths.get(&key) {
            return Some(path.clone());
        }
        let path = build()?;
        if paths.len() >= KEPT_PATHS {
            paths.clear();
        }
        paths.insert(key, path.clone());
        Some(path)
    });
    if let Some(path) = path {
        window.paint_path(path, tint);
    }
}

/// Where `bounds` is, exactly enough to tell one drawing from another.
fn spot(bounds: Bounds<Pixels>) -> [u32; 4] {
    [
        f32::from(bounds.origin.x).to_bits(),
        f32::from(bounds.origin.y).to_bits(),
        f32::from(bounds.size.width).to_bits(),
        f32::from(bounds.size.height).to_bits(),
    ]
}

/// Where a tick of a meter starts after the one before it.
const TICK_STEP: f32 = 4.;
const DASH: f32 = 3.;

/// A row of fine ticks lit from the left as far as `share` of the whole, the rest faint.
pub fn ticks(share: f32, lit: Hsla, unlit: Hsla, height: f32) -> impl IntoElement {
    let share = share.clamp(0., 1.);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let width = f32::from(bounds.size.width);
            let count = (width / TICK_STEP).floor().max(0.) as usize;
            let on = (count as f32 * share).round() as usize;
            let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
            let tick = |at: usize| {
                let x = bounds.origin.x + px(at as f32 * TICK_STEP + 0.5);
                (point(x, top), point(x, bottom))
            };
            paint_kept(window, ("lit", spot(bounds), on), lit, || {
                lines((0..on).map(tick))
            });
            paint_kept(window, ("unlit", spot(bounds), on, count), unlit, || {
                lines((on..count).map(tick))
            });
        },
    )
    .w_full()
    .h(px(height))
}

/// One line over time, each value a share of its whole, oldest first, spread across `start` to
/// `end` by when each was taken.
pub struct Series {
    pub points: Vec<(u64, f32)>,
    pub start: u64,
    pub end: u64,
    pub tint: Hsla,
    /// Moments worth a mark, with the words over the mark where there are any.
    pub marks: Vec<(u64, Option<String>)>,
    /// A share drawn as a dashed line across, such as the 90% a host starts to struggle past.
    pub limit: Option<f32>,
}

impl Series {
    fn x(&self, at: u64) -> f32 {
        let span = self.end.saturating_sub(self.start).max(1) as f32;
        (at.saturating_sub(self.start) as f32 / span).clamp(0., 1.)
    }
}

/// `series` as a filled line `height` tall with its limit dashed, its marks and a dot where it
/// ends.
pub fn history(series: Series, height: f32, t: &Tokens) -> AnyElement {
    let (tint, rule) = (series.tint, t.text_tertiary.opacity(0.55));
    let (mark_line, label_ink) = (t.text_tertiary.opacity(0.7), t.text_secondary);
    let placed: Vec<(f32, f32)> = (series.points.iter())
        .map(|(at, share)| (series.x(*at), share.clamp(0., 1.)))
        .collect();
    let marks: Vec<(f32, Option<String>)> = (series.marks.iter())
        .map(|(at, words)| (series.x(*at), words.clone()))
        .collect();
    let end = placed.last().copied();
    let (limit, line) = (series.limit, placed.clone());
    let lines: Vec<f32> = marks.iter().map(|(x, _)| *x).collect();
    let painted = canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            if let Some(limit) = limit {
                let y = bounds.origin.y + bounds.size.height * (1. - limit);
                dashes_across(window, bounds, y, rule);
            }
            for x in &lines {
                dashes_down(
                    window,
                    bounds,
                    bounds.origin.x + bounds.size.width * *x,
                    mark_line,
                );
            }
            paint_line(window, bounds, &line, tint);
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .w_full()
        .h(px(height))
        .child(painted)
        .children(marks.into_iter().filter_map(|(x, words)| {
            let words = words?;
            Some(
                div()
                    .absolute()
                    .top(px(-15.))
                    .map(|label| match x > 0.6 {
                        true => label.right(relative(1. - x)).mr(px(5.)),
                        false => label.left(relative(x)).ml(px(5.)),
                    })
                    .child(
                        ui::text(words, TEXT_3XS, FontWeight::NORMAL)
                            .font_family(crate::theme::MONO)
                            .text_color(label_ink),
                    ),
            )
        }))
        .children(end.map(|(x, y)| {
            div()
                .absolute()
                .left(relative(x))
                .top(relative(1. - y))
                .ml(px(-3.))
                .mt(px(-3.))
                .size(px(6.))
                .rounded_full()
                .bg(tint)
        }))
        .into_any_element()
}

fn paint_line(window: &mut Window, bounds: Bounds<Pixels>, line: &[(f32, f32)], tint: Hsla) {
    let at = |(x, y): (f32, f32)| {
        point(
            bounds.origin.x + bounds.size.width * x,
            bounds.origin.y + bounds.size.height * (1. - y),
        )
    };
    let (Some(first), Some(last)) = (line.first().copied(), line.last().copied()) else {
        return;
    };
    let drawn: Vec<[u32; 2]> = (line.iter())
        .map(|(x, y)| [x.to_bits(), y.to_bits()])
        .collect();
    let bottom = bounds.origin.y + bounds.size.height;
    paint_kept(
        window,
        ("area", spot(bounds), &drawn),
        tint.opacity(0.1),
        || {
            let mut area = PathBuilder::fill();
            area.move_to(point(at(first).x, bottom));
            for point in line {
                area.line_to(at(*point));
            }
            area.line_to(point(at(last).x, bottom));
            area.close();
            area.build().ok()
        },
    );
    paint_kept(window, ("stroke", spot(bounds), &drawn), tint, || {
        let mut stroke = PathBuilder::stroke(px(1.25));
        stroke.move_to(at(first));
        for point in &line[1..] {
            stroke.line_to(at(*point));
        }
        stroke.build().ok()
    });
}

fn dashes_across(window: &mut Window, bounds: Bounds<Pixels>, y: Pixels, tint: Hsla) {
    let width = f32::from(bounds.size.width);
    let y = y + px(0.5);
    let dashes = (0..)
        .map(|at| at as f32 * DASH * 2.)
        .take_while(|x| *x < width);
    let dash = |x: f32| {
        let x = bounds.origin.x + px(x);
        (
            point(x, y),
            point(x + px(DASH.min(width - f32::from(x - bounds.origin.x))), y),
        )
    };
    paint_kept(
        window,
        ("across", spot(bounds), f32::from(y).to_bits()),
        tint,
        || lines(dashes.map(dash)),
    );
}

fn dashes_down(window: &mut Window, bounds: Bounds<Pixels>, x: Pixels, tint: Hsla) {
    let height = f32::from(bounds.size.height);
    let x = x + px(0.5);
    let dashes = (0..)
        .map(|at| at as f32 * DASH * 2.)
        .take_while(|y| *y < height);
    let dash = |y: f32| {
        let top = bounds.origin.y + px(y);
        (point(x, top), point(x, top + px(DASH.min(height - y))))
    };
    paint_kept(
        window,
        ("down", spot(bounds), f32::from(x).to_bits()),
        tint,
        || lines(dashes.map(dash)),
    );
}

/// Many short 1px lines as one stroked path, which the window takes in once rather than line by
/// line.
fn lines(pairs: impl Iterator<Item = (Point<Pixels>, Point<Pixels>)>) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::stroke(px(1.));
    let mut any = false;
    for (from, to) in pairs {
        any = true;
        path.move_to(from);
        path.line_to(to);
    }
    any.then(|| path.build().ok()).flatten()
}
