//! A branch's history: its commits newest first, each on a lane of the graph that shows
//! where it branched and merged, and the card that heads a commit's review.

use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, hsla, point, px, radians, relative, Anchor, AnyElement, App, Bounds,
    ClipboardItem, Context, Corners, Div, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, PathBuilder, Pixels, SharedString, StatefulInteractiveElement,
    Styled, Transformation, WeakEntity, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_core::git::log::GitCommit;

use super::GitPane;
use crate::core::Target;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    SPACE_4, TEXT_2XS, TEXT_3XS, TEXT_LG, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::toast;
use crate::ui;
use crate::workbench::{AgentRow, Workbench};

pub const ROW_HEIGHT: f32 = 32.;
const LANE_WIDTH: f32 = 14.;
const FIRST_LANE: f32 = 14.;
const NODE: f32 = 3.;
const GUTTER_PAD: f32 = 18.;

/// A line of the graph through one row, coloured by its lane, or the unpushed colour
/// while it leads to commits not yet pushed.
#[derive(Clone, Copy)]
struct Edge {
    lane: usize,
    unpushed: bool,
}

/// One commit's row of the graph: its node, the lines that land on it from above, the
/// lines that pass it, and the lines to its parents below.
#[derive(Clone)]
pub struct Lanes {
    lane: usize,
    head: bool,
    unpushed: bool,
    merges: Vec<(usize, Edge)>,
    through: Vec<Edge>,
    branches: Vec<(usize, Edge)>,
}

pub struct Graph {
    pub rows: Vec<Lanes>,
    pub width: f32,
}

/// Lays the commits out on lanes as today's graph does: a commit takes the lane that
/// waits for it, else the first free one, and its parents wait on lanes below it.
pub fn graph(commits: &[GitCommit]) -> Graph {
    let visible: std::collections::HashSet<&str> = commits
        .iter()
        .map(|commit| commit.full_hash.as_str())
        .collect();
    let mut drawn: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut up: Vec<bool> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut widest = 0;
    let free = |lanes: &mut Vec<Option<&str>>, up: &mut Vec<bool>| {
        lanes.iter().position(Option::is_none).unwrap_or_else(|| {
            lanes.push(None);
            up.push(false);
            lanes.len() - 1
        })
    };
    for commit in commits {
        let hash = commit.full_hash.as_str();
        let expecting: Vec<usize> = (0..lanes.len())
            .filter(|lane| lanes[*lane] == Some(hash))
            .collect();
        let lane = match expecting.first() {
            Some(lane) => *lane,
            None => free(&mut lanes, &mut up),
        };
        let merges = expecting
            .iter()
            .map(|from| {
                (
                    *from,
                    Edge {
                        lane: *from,
                        unpushed: up[*from],
                    },
                )
            })
            .collect();
        let through = (0..lanes.len())
            .filter(|other| *other != lane && lanes[*other].is_some_and(|waits| waits != hash))
            .map(|other| Edge {
                lane: other,
                unpushed: up[other],
            })
            .collect();
        for from in &expecting {
            lanes[*from] = None;
            up[*from] = false;
        }
        lanes[lane] = None;
        up[lane] = false;
        let parents: Vec<&str> = commit
            .parents
            .iter()
            .map(String::as_str)
            .filter(|parent| visible.contains(parent) && !drawn.contains(parent))
            .collect();
        let mut branches = Vec::new();
        for (index, parent) in parents.iter().enumerate() {
            let to = match lanes.iter().position(|waits| *waits == Some(*parent)) {
                Some(to) => to,
                None if index == 0 => {
                    lanes[lane] = Some(parent);
                    up[lane] = commit.unpushed;
                    lane
                }
                None => {
                    let to = free(&mut lanes, &mut up);
                    lanes[to] = Some(parent);
                    up[to] = commit.unpushed;
                    to
                }
            };
            branches.push((
                to,
                Edge {
                    lane: to,
                    unpushed: commit.unpushed,
                },
            ));
        }
        rows.push(Lanes {
            lane,
            head: commit.refs.iter().any(|name| name == "HEAD"),
            unpushed: commit.unpushed,
            merges,
            through,
            branches,
        });
        drawn.insert(hash);
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
            up.pop();
        }
        widest = widest.max(lanes.len()).max(lane + 1);
    }
    Graph {
        rows,
        width: FIRST_LANE + widest.max(1).saturating_sub(1) as f32 * LANE_WIDTH + GUTTER_PAD,
    }
}

fn lane_x(lane: usize) -> f32 {
    FIRST_LANE + lane as f32 * LANE_WIDTH
}

/// The graph's colours: a lane's by its number, what is not yet pushed, and the ground a
/// node is cut out of.
#[derive(Clone)]
pub struct Inks {
    lanes: Rc<[Hsla]>,
    unpushed: Hsla,
    ground: Hsla,
}

impl Inks {
    pub fn new(t: &Tokens) -> Self {
        let extra = [0x7cc5ff, 0xffd166, 0x9b8cff].map(|rgb| gpui_kit::rgb(rgb).into());
        Self {
            lanes: [t.accent, t.live, t.cmd, t.warn, t.danger]
                .into_iter()
                .chain(extra)
                .collect(),
            unpushed: t.warn,
            ground: t.surface_sunken,
        }
    }

    fn edge(&self, edge: Edge) -> Hsla {
        if edge.unpushed {
            self.unpushed
        } else {
            self.lanes[edge.lane % self.lanes.len()]
        }
    }
}

/// One row's piece of the graph, with a ring round its node when the row is chosen.
fn paint(lanes: &Lanes, inks: &Inks, chosen: bool, bounds: Bounds<Pixels>, window: &mut Window) {
    let origin = bounds.origin;
    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
    let (top, middle, bottom) = (0., ROW_HEIGHT / 2., ROW_HEIGHT);
    let pull = ROW_HEIGHT * 0.42;
    let node = lane_x(lanes.lane);
    let mut stroke = |from: (f32, f32),
                      to: (f32, f32),
                      curve: Option<((f32, f32), (f32, f32))>,
                      colour: Hsla| {
        let mut path = PathBuilder::stroke(px(1.5));
        path.move_to(at(from.0, from.1));
        match curve {
            Some((a, b)) => path.cubic_bezier_to(at(to.0, to.1), at(a.0, a.1), at(b.0, b.1)),
            None => path.line_to(at(to.0, to.1)),
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, colour.opacity(0.7));
        }
    };
    for edge in &lanes.through {
        let x = lane_x(edge.lane);
        stroke((x, top), (x, bottom), None, inks.edge(*edge));
    }
    for (from, edge) in &lanes.merges {
        let x = lane_x(*from);
        let curve = (*from != lanes.lane).then_some(((x, top + pull), (node, middle - pull)));
        stroke((x, top), (node, middle), curve, inks.edge(*edge));
    }
    for (to, edge) in &lanes.branches {
        let x = lane_x(*to);
        let curve = (*to != lanes.lane).then_some(((node, middle + pull), (x, bottom - pull)));
        stroke((node, middle), (x, bottom), curve, inks.edge(*edge));
    }
    let colour = if lanes.unpushed {
        inks.unpushed
    } else {
        inks.lanes[lanes.lane % inks.lanes.len()]
    };
    let disc = |radius: f32, colour: Hsla, window: &mut Window| {
        let centre = at(node, middle);
        let bounds = Bounds::new(
            point(centre.x - px(radius), centre.y - px(radius)),
            gpui_kit::size(px(radius * 2.), px(radius * 2.)),
        );
        window.paint_quad(gpui_kit::fill(bounds, colour).corner_radii(Corners::all(px(radius))));
    };
    disc(NODE + 1.5, inks.ground, window);
    if lanes.head {
        disc(NODE + 1., colour, window);
        disc(NODE - 0.5, inks.ground, window);
        disc(1.6, colour, window);
    } else {
        disc(NODE, colour, window);
    }
    if chosen {
        let centre = at(node, middle);
        let radius = NODE + 3.;
        let mut ring = PathBuilder::stroke(px(1.));
        ring.move_to(point(centre.x + px(radius), centre.y));
        ring.arc_to(
            point(px(radius), px(radius)),
            px(0.),
            false,
            true,
            point(centre.x - px(radius), centre.y),
        );
        ring.arc_to(
            point(px(radius), px(radius)),
            px(0.),
            false,
            true,
            point(centre.x + px(radius), centre.y),
        );
        if let Ok(ring) = ring.build() {
            window.paint_path(ring, hsla(0., 0., 1., 0.75));
        }
    }
}

/// A conventional-commit prefix such as `feat(git): ` repeats down the whole list, so a
/// row shows what follows it.
fn without_scope(subject: &str) -> &str {
    let kind = subject
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(subject.len());
    if kind == 0 {
        return subject;
    }
    let mut rest = &subject[kind..];
    if let Some(scoped) = rest.strip_prefix('(') {
        match scoped.find(')') {
            Some(close) => rest = &scoped[close + 1..],
            None => return subject,
        }
    }
    rest = rest.strip_prefix('!').unwrap_or(rest);
    match rest.strip_prefix(':') {
        Some(said) if said.starts_with(char::is_whitespace) => {
            let said = said.trim_start();
            if said.is_empty() {
                subject
            } else {
                said
            }
        }
        _ => subject,
    }
}

/// A ref a commit carries: the branch HEAD is on, another branch, a remote's, or a tag.
fn ref_badge(label: &str, cx: &App) -> Div {
    let t = Tokens::get(cx);
    let blue: Hsla = gpui_kit::rgb(0x7cc5ff).into();
    let (text, ink, ground, edge) = if let Some(branch) = label.strip_prefix("HEAD -> ") {
        (
            branch,
            t.accent,
            Some(t.surface_selected),
            t.border_selected,
        )
    } else if label == "HEAD" {
        (label, t.accent, Some(t.surface_selected), t.border_selected)
    } else if let Some(tag) = label.strip_prefix("tag: ") {
        (tag, t.warn, Some(t.warn.opacity(0.08)), t.warn.opacity(0.3))
    } else if label.contains('/') {
        (label, blue, Some(blue.opacity(0.08)), blue.opacity(0.3))
    } else {
        (
            label,
            t.live,
            Some(t.live.opacity(0.07)),
            t.live.opacity(0.28),
        )
    };
    div()
        .flex_none()
        .min_w(px(48.))
        .max_w(px(108.))
        .flex()
        .items_center()
        .gap(SPACE_1)
        .pl(SPACE_1H)
        .pr(px(7.))
        .py(SPACE_0)
        .border_1()
        .border_color(edge)
        .rounded(RADIUS_2)
        .when_some(ground, |badge, ground| badge.bg(ground))
        .text_size(crate::theme::ui_text(TEXT_3XS))
        .line_height(crate::theme::ui_text(TEXT_3XS))
        .text_color(ink)
        .child(div().size(px(5.)).flex_none().rounded_full().bg(ink))
        .child(div().min_w_0().truncate().child(text.to_owned()))
}

/// One commit's row: its piece of the graph, its author, the ref it carries, what it
/// says, and when.
pub fn commit_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    index: usize,
    inks: &Inks,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let (Some(commit), Some(lanes)) = (pane.commits.get(index), pane.graph.rows.get(index)) else {
        return div().into_any_element();
    };
    let gutter = pane.graph.width;
    let chosen = index == pane.commit;
    let hovered =
        pane.hovered.as_ref() == Some(&SharedString::from(format!("commit:{}", commit.full_hash)));
    let refs: Vec<&str> = commit
        .refs
        .iter()
        .filter(|name| *name != "HEAD")
        .map(String::as_str)
        .collect();
    let (graph_inks, graph) = (inks.clone(), lanes.clone());
    let key = SharedString::from(format!("commit:{}", commit.full_hash));
    let choosing = view.clone();
    div()
        .id(key.clone())
        .relative()
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(gutter))
        .pr(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .rounded(RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_secondary
        })
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|row| {
            if chosen {
                row.bg(t.surface_chosen)
            } else {
                row.hover(move |row| row.bg(t.surface_hover))
            }
        })
        .on_hover(super::hover(view, key))
        .on_click(move |_, window, cx| {
            choosing.update(cx, |pane, cx| pane.choose_commit(index, window, cx))
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint(&graph, &graph_inks, chosen, bounds, window),
            )
            .absolute()
            .left_0()
            .top_0()
            .w(px(gutter))
            .h(px(ROW_HEIGHT)),
        )
        .child(pane.author_face(&commit.author, &commit.author_email, 15., cx))
        .children(refs.first().map(|name| ref_badge(name, cx)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::theme::ui_text(TEXT_SM))
                .font_weight(FontWeight::MEDIUM)
                .child(without_scope(&commit.subject).to_owned()),
        )
        .children(
            commit_agents(pane, &commit.full_hash, true, cx)
                .map(|agents| agents.max_w(relative(0.4))),
        )
        .when(chosen || hovered, |row| {
            row.child(
                div()
                    .flex_none()
                    .font_family(theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .child(commit.hash.clone()),
            )
        })
        .child(
            div()
                .flex_none()
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .text_color(t.text_tertiary)
                .child(commit.date.clone()),
        )
        .into_any_element()
}

/// The card heading a commit's review: what it says, who wrote it and when, and its hash
/// with the refs it carries, on a panel above the review.
pub fn commit_card(
    pane: &GitPane,
    commit: &GitCommit,
    pushed: bool,
    cx: &mut Context<GitPane>,
) -> Div {
    let t = Tokens::get(cx).clone();
    let fact = |label: &'static str, value: AnyElement| {
        div()
            .flex()
            .items_center()
            .min_h(px(22.))
            .gap(SPACE_3)
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_color(t.text_tertiary)
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_1H)
                    .min_w_0()
                    .text_color(t.text_secondary)
                    .child(value),
            )
    };
    let chip = |text: String, colour: Hsla, edge: Hsla| {
        div()
            .h(px(18.))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .border_1()
            .border_color(edge)
            .rounded(px(4.))
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(colour)
            .child(text)
    };
    let (full, short) = (commit.full_hash.clone(), commit.hash.clone());
    let mut hash = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(
            div()
                .font_family(theme::MONO)
                .text_color(t.text_secondary)
                .child(commit.hash.clone()),
        )
        .child(
            ui::IconButton::new("copy-hash", crate::icons::Icon::Copy, "Copy hash")
                .glyph(12.)
                .on_click(move |_, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                    window.push_notification(toast::success(format!("Copied {short}")), cx);
                }),
        );
    if commit.unpushed {
        hash = hash.child(chip("not pushed".into(), t.warn, t.warn.opacity(0.35)));
    } else if pushed {
        hash = hash.child(chip("pushed".into(), t.text_secondary, t.border));
    }
    for name in commit.refs.iter().filter(|name| *name != "HEAD") {
        let shown = name
            .strip_prefix("HEAD -> ")
            .or_else(|| name.strip_prefix("tag: "))
            .unwrap_or(name);
        hash = hash.child(chip(shown.to_owned(), t.text_secondary, t.border));
    }
    let body = div()
        .px(SPACE_4)
        .pt(SPACE_3H)
        .pb(SPACE_3)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_LG))
                .line_height(crate::theme::ui_text(21.6))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child(commit.subject.clone()),
        )
        .child(
            div()
                .mt(SPACE_3)
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .text_size(crate::theme::ui_text(TEXT_2XS))
                .child(fact(
                    "Author",
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(pane.author_face(&commit.author, &commit.author_email, 20., cx))
                        .child(commit.author.clone())
                        .children(commit_agents(pane, &commit.full_hash, false, cx))
                        .into_any_element(),
                ))
                .child(fact(
                    "Date",
                    div().child(commit.date.clone()).into_any_element(),
                ))
                .child(fact("Commit", hash.into_any_element())),
        );
    ui::panel(cx)
        .flex_none()
        .font_family(theme::UI)
        .child(body)
        .child(commit_foot(commit, cx))
}

/// The foot of a commit's card: a branch from it, and the menu that undoes it.
fn commit_foot(commit: &GitCommit, cx: &mut Context<GitPane>) -> Div {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    let foot_button = |id: &'static str, icon: Icon, label: &'static str| {
        gpui_kit::base::Button::new(id)
            .accessibility_label(label)
            .h(px(26.))
            .px(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
            .child(ui::icon(icon, 12.).text_color(t.text_tertiary))
            .child(label)
    };
    let branched = commit.hash.clone();
    let (reset, reverted) = (commit.hash.clone(), commit.hash.clone());
    div()
        .flex()
        .items_center()
        .gap(SPACE_0)
        .px(SPACE_2)
        .py(SPACE_1H)
        .border_t_1()
        .border_color(t.border)
        .child(
            foot_button("commit-branch", Icon::Git, "Branch from here").on_click(
                cx.listener(move |pane, _, _, cx| pane.branch_from(branched.clone(), cx)),
            ),
        )
        .child(ui::menu_button(
            "commit-undo",
            Anchor::BottomLeft,
            foot_button("commit-undo-button", Icon::Discard, "Undo this commit").child(
                ui::icon(Icon::Chevron, 9.)
                    .text_color(t.text_tertiary)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2))),
            ),
            move |popup, _, cx| {
                let (resetting, reverting) = (view.clone(), view.clone());
                let (reset, reverted) = (reset.clone(), reverted.clone());
                popup
                    .item(
                        ui::MenuRow::new("Reset to here…")
                            .icon(Icon::Discard)
                            .hint("r")
                            .item(cx)
                            .on_click(move |_, _, cx| {
                                resetting.update(cx, |pane, cx| pane.reset_to(reset.clone(), cx));
                            }),
                    )
                    .item(
                        ui::MenuRow::new("Revert…")
                            .icon(Icon::Discard)
                            .hint("v")
                            .danger(true)
                            .item(cx)
                            .on_click(move |_, _, cx| {
                                reverting.update(cx, |pane, cx| pane.revert(reverted.clone(), cx));
                            }),
                    )
            },
        ))
}

/// An agent a commit's signed footer names, as the core's `commitOwners` answers.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitOwner {
    name: String,
    cli: String,
    cli_version: Option<String>,
    model: Option<String>,
    sikemux: Option<String>,
    chat: String,
    title: Option<String>,
    /// The agent that has its session open now.
    open: Option<String>,
    session_id: Option<String>,
    /// The folder of the project its session ran in.
    project: Option<String>,
}

impl CommitOwner {
    fn describe(&self) -> String {
        let who = match &self.title {
            Some(title) => format!("{title} — {}", self.name),
            None => self.name.clone(),
        };
        let reach = if self.open.is_some() {
            "open now"
        } else if self.session_id.is_some() {
            "click to resume"
        } else {
            "its session is gone"
        };
        let cli = match &self.cli_version {
            Some(version) => format!("{} {version}", self.cli),
            None => self.cli.clone(),
        };
        let ran: Vec<String> = [
            Some(cli),
            self.model.clone(),
            self.sikemux
                .as_ref()
                .map(|version| format!("Sikemux {version}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        format!("{who} ({}) wrote this · {reach}", ran.join(", "))
    }

    /// Brings forward the agent that has the session open, else takes the session up again
    /// in the project it ran in, on the core the history came from.
    fn open(
        &self,
        bench: &WeakEntity<Workbench>,
        core: &Target,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(agent) = &self.open {
            if let Some(bench) = bench.upgrade() {
                bench.read(cx).open_agent(agent);
            }
            return;
        }
        let (Some(session), Some(project)) = (&self.session_id, &self.project) else {
            return;
        };
        let params = json!({ "provider": self.cli, "sessionId": session, "project": project });
        let (answer, core) = (core.request("resumeSession", params), core.clone());
        window
            .spawn(cx, async move |cx| match answer.await {
                Ok(started) => {
                    let tab = json!({ "project": started["project"], "kind": "agent", "target": started["agentId"] });
                    core.call("openTab", tab);
                }
                Err(error) => {
                    let _ = cx.update(|window, cx| {
                        window.push_notification(toast::error(error), cx)
                    });
                }
            })
            .detach();
    }
}

/// The agents a commit's signed footer names, each opening its session; `compact` shows
/// only their marks when there are several.
fn commit_agents(pane: &GitPane, hash: &str, compact: bool, cx: &App) -> Option<Div> {
    let owners = pane
        .commit_owners
        .get(hash)
        .filter(|owners| !owners.is_empty())?;
    let t = Tokens::get(cx).clone();
    let rows: Vec<AgentRow> = pane.bench.upgrade().map_or_else(Vec::new, |bench| {
        let bench = bench.read(cx);
        bench
            .projects()
            .iter()
            .flat_map(|project| bench.agents_of(&project.id))
            .collect()
    });
    let marks_only = compact && owners.len() > 1;
    let chips = owners.iter().enumerate().map(|(index, owner)| {
        let shown = owner.open.as_ref().unwrap_or(&owner.chat);
        let mark = match rows.iter().find(|row| &row.id == shown) {
            Some(row) => row.mark(11., cx),
            None => ui::mark(&owner.cli, 11.),
        };
        let label = match &owner.title {
            Some(title) => div()
                .flex()
                .min_w_0()
                .child(div().min_w_0().truncate().child(title.clone()))
                .child(
                    div()
                        .flex_none()
                        .ml(SPACE_1H)
                        .text_color(t.text_tertiary)
                        .child(format!("@{}", owner.name)),
                ),
            None => div().min_w_0().truncate().child(owner.name.clone()),
        };
        let reachable =
            owner.open.is_some() || (owner.session_id.is_some() && owner.project.is_some());
        let (bench, core, opened) = (pane.bench.clone(), pane.core.clone(), owner.clone());
        div()
            .id(SharedString::from(format!("commit-agent:{hash}:{index}")))
            .h(px(18.))
            .min_w(px(25.))
            .px(px(if marks_only { 4. } else { 6. }))
            .flex()
            .items_center()
            .gap(SPACE_1)
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .font_weight(FontWeight::NORMAL)
            .text_color(t.text_secondary)
            .whitespace_nowrap()
            .tooltip(ui::tooltip(owner.describe()))
            .when(reachable, |chip| {
                chip.cursor_pointer()
                    .hover(move |chip| chip.bg(t.surface_hover).text_color(t.text_primary))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        opened.open(&bench, &core, window, cx);
                    })
            })
            .child(mark)
            .when(!marks_only, |chip| chip.child(label))
    });
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .min_w_0()
            .children(chips),
    )
}
