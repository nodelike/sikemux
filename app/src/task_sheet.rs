//! The sheet over the Tasks view for one task the person asked: where it stands and who has it,
//! what the person sent under it, and what opened on its agent's desk.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, Context, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    WeakEntity,
};
use std::path::PathBuf;

use sikemux_calls::call;
use sikemux_calls::params::AskParams;
use sikemux_view::now::{AskSaid, AskStand, NowAsk, NowGroup, ProjectNow};
use sikemux_view::task_list::TaskOf;

use crate::icons::Icon;
use crate::picture::Picture;
use crate::theme::{
    self, radius_2, radius_3, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, TEXT_XS,
};
use crate::ui::{self, Clicks, IconButton};
use crate::workbench::Workbench;

const SHEET_W: f32 = 340.;
const SAID_THUMB_W: f32 = 120.;
const SAID_THUMB_H: f32 = 80.;

#[derive(Clone, Copy, PartialEq)]
enum Lane {
    Nobody,
    AsksYou,
    Moving,
    Done,
}

impl Lane {
    fn of(ask: &NowAsk, now: &ProjectNow) -> Option<Lane> {
        match &ask.stand {
            AskStand::Nobody => Some(Lane::Nobody),
            AskStand::Working { agents } if agents.iter().any(|agent| waits(now, agent)) => {
                Some(Lane::AsksYou)
            }
            AskStand::Working { .. } => Some(Lane::Moving),
            AskStand::OnBranch | AskStand::InYourBuild => Some(Lane::Done),
            AskStand::Unknown => None,
        }
    }
}

fn waits(now: &ProjectNow, agent: &str) -> bool {
    (now.rows.iter()).any(|row| row.agent == agent && row.group == NowGroup::WaitingOnYou)
}

impl Workbench {
    fn board_now(&self, project: &str) -> Option<&ProjectNow> {
        (self.view_at(&self.place())?.now.iter()).find(|now| now.project == project)
    }

    pub(crate) fn open_sheet(&mut self, project: &str, id: u64, cx: &mut Context<Self>) {
        self.task_sheet = Some(Sheet {
            project: project.to_owned(),
            id,
            said: Vec::new(),
            read_at: None,
        });
        self.keep_task_sheet(cx);
        self.parts.redraw_stage(cx);
        cx.notify();
    }

    fn close_sheet(&mut self, cx: &mut Context<Self>) {
        if self.task_sheet.take().is_some() {
            self.parts.redraw_stage(cx);
            cx.notify();
        }
    }

    /// Moves the sheet `by` tasks along the ones the person asked, in the Tasks view's order.
    fn step_sheet(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(sheet) = self.task_sheet.as_ref() else {
            return;
        };
        let project = sheet.project.clone();
        let order: Vec<u64> = (self.feeds.at(&self.place()))
            .and_then(|feed| feed.task_list.as_ref())
            .into_iter()
            .flat_map(|list| &list.rows)
            .filter(|row| row.asked)
            .filter_map(|row| match row.of {
                TaskOf::Task { id } => Some(id),
                TaskOf::Call { .. } => None,
            })
            .collect();
        let Some(at) = order.iter().position(|id| *id == sheet.id) else {
            return;
        };
        let Some(&next) = at.checked_add_signed(by).and_then(|at| order.get(at)) else {
            return;
        };
        self.open_sheet(&project, next, cx);
    }

    /// The view moved: the sheet closes once its task is gone, and reads what was said under it
    /// again once more was.
    pub(crate) fn keep_task_sheet(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = self.task_sheet.as_ref() else {
            return;
        };
        let shown = (self.board_now(&sheet.project))
            .and_then(|now| now.asks.iter().find(|ask| ask.id == sheet.id));
        let Some(more) = shown.map(|ask| ask.more) else {
            return self.close_sheet(cx);
        };
        if sheet.read_at == Some(more) {
            return;
        }
        let (id, project) = (sheet.id, sheet.project.clone());
        if let Some(sheet) = self.task_sheet.as_mut() {
            sheet.read_at = Some(more);
        }
        let said = self
            .target()
            .request_typed::<call::AskSaid>(AskParams { project, id });
        cx.spawn(async move |bench, cx| {
            let Ok(said) = said.await else {
                return;
            };
            let _ = bench.update(cx, |bench, cx| {
                if let Some(sheet) = bench.task_sheet.as_mut().filter(|sheet| sheet.id == id) {
                    sheet.said = said;
                    bench.parts.redraw_stage(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// The task the sheet shows, and what the person sent under it as the core last said.
pub(crate) struct Sheet {
    project: String,
    id: u64,
    said: Vec<AskSaid>,
    /// How many more messages the task had when `said` was read.
    read_at: Option<u32>,
}

pub(crate) fn sheet_over(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let sheet = bench.task_sheet.as_ref()?;
    let now = bench.board_now(&sheet.project)?;
    let ask = now.asks.iter().find(|ask| ask.id == sheet.id)?;
    let handle = cx.weak_entity();
    Some(sheet_of(bench, sheet, ask, Lane::of(ask, now), &handle, cx))
}

enum Link {
    Page(String),
    File(PathBuf),
}

/// The pages and files the task's agent opened on its desk while on it, the first two as chips
/// that open on that agent's desk with one click, and how many more.
fn links(bench: &Workbench, ask: &NowAsk, handle: &WeakEntity<Workbench>, cx: &App) -> Option<Div> {
    const SHOWN: usize = 2;
    let t = Tokens::get(cx);
    let count = ask.pages.len() + ask.files.len();
    if count == 0 {
        return None;
    }
    let opener = |link: Link| {
        let (handle, agent) = (handle.clone(), ask.to.clone());
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            cx.stop_propagation();
            let _ = handle.update(cx, |bench, cx| match &link {
                Link::Page(url) => {
                    bench.open_on_desk(&agent, url, cx);
                }
                Link::File(path) => bench.open_file_on_desk(&agent, path.clone(), None, None, cx),
            });
        }
    };
    let pages = ask.pages.iter().map(|page| {
        let chip = crate::chat::context_chip(
            format!("ask-{}-page-{}", ask.id, page.id),
            &page.url,
            &page.title,
            Some(&bench.asks),
            t,
        );
        (chip, opener(Link::Page(page.url.clone())))
    });
    let files = ask.files.iter().map(|path| {
        let path = PathBuf::from(path);
        let id = format!("ask-{}-file-{}", ask.id, path.display());
        let chip = crate::chat::file_chip(id, &path, t, cx);
        (chip, opener(Link::File(path)))
    });
    let chips = (pages.chain(files).take(SHOWN).enumerate()).map(|(at, (chip, open))| {
        div()
            .id(SharedString::from(format!("ask-{}-link-{at}", ask.id)))
            .min_w_0()
            .clicks(open)
            .child(chip)
    });
    let more = count.saturating_sub(SHOWN);
    Some(
        div()
            .px(SPACE_3)
            .pt(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .overflow_hidden()
            .children(chips)
            .when(more > 0, |row| {
                row.child(
                    ui::text(format!("+{more}"), TEXT_XS, FontWeight::MEDIUM)
                        .flex_none()
                        .font_features(theme::tabular())
                        .text_color(t.text_tertiary),
                )
            }),
    )
}

/// How far the task has gone, in its lane's words and tint; none while it moves, where each
/// agent's own mark says how it stands.
fn stand(ask: &NowAsk, lane: Lane, t: &Tokens) -> Option<Div> {
    let (icon, words, tint, weight): (Option<AnyElement>, &str, Hsla, FontWeight) =
        match (&ask.stand, lane) {
            (_, Lane::Nobody) => (None, t!("Nobody on it"), t.warn, FontWeight::MEDIUM),
            (_, Lane::AsksYou) => (None, t!("Asks you"), t.accent, FontWeight::MEDIUM),
            (AskStand::InYourBuild, _) => (
                Some(
                    ui::icon(Icon::Check, 12.)
                        .text_color(t.live)
                        .into_any_element(),
                ),
                t!("In your build"),
                t.live,
                FontWeight::NORMAL,
            ),
            (AskStand::OnBranch, _) => (
                None,
                t!("Not in your build yet"),
                t.text_tertiary,
                FontWeight::NORMAL,
            ),
            _ => return None,
        };
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(5.))
            .children(icon)
            .child(ui::text(words, TEXT_XS, weight).text_color(tint)),
    )
}

/// What the person wrote to `to`, with the files and agents it names as the chat's chips.
fn written(
    bench: &Workbench,
    to: &str,
    key: &str,
    text: &str,
    images: &[String],
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    let agents = crate::chat::named_agents(bench, to);
    let named = crate::chat::named(text, images, &[], &agents);
    let pieces = crate::stash::pieces(text, &named, &[]);
    crate::stash::sentence(Some(key), &pieces, bench, t, cx)
}

/// The open task over the Tasks view's right side: its name, where it stands and who has it, what the
/// person sent under it, what opened on its agent's desk, and the way to its chat.
fn sheet_of(
    bench: &Workbench,
    sheet: &Sheet,
    ask: &NowAsk,
    lane: Option<Lane>,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let on = |act: fn(&mut Workbench, &mut Context<Workbench>)| {
        let handle = handle.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            cx.stop_propagation();
            let _ = handle.update(cx, act);
        }
    };
    let step = |by: isize| {
        let handle = handle.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let _ = handle.update(cx, |bench, cx| bench.step_sheet(by, cx));
        }
    };
    let chat = {
        let (handle, id) = (handle.clone(), ask.id);
        move |_: &gpui_kit::ClickEvent, _: &mut gpui_kit::Window, cx: &mut App| {
            let _ = handle.update(cx, |bench, cx| {
                bench.close_sheet(cx);
                bench.open_task(id, cx);
            });
        }
    };
    let caps = div()
        .flex()
        .items_center()
        .gap(SPACE_1)
        .child(ui::caps(&t!("Task #{id}", id = ask.id), cx).flex_1())
        .child(
            IconButton::new("ask-sheet-back", Icon::Chevron, t!("Previous task"))
                .small()
                .turned(std::f32::consts::PI)
                .on_click(step(-1)),
        )
        .child(
            IconButton::new("ask-sheet-on", Icon::Chevron, t!("Next task"))
                .small()
                .on_click(step(1)),
        )
        .child(
            IconButton::new("ask-sheet-close", Icon::Close, t!("Close"))
                .small()
                .on_click(on(Workbench::close_sheet)),
        );
    let named = match &ask.name {
        Some(name) => name.clone().into_any_element(),
        None => {
            let key = format!("ask-sheet-{}-title", ask.id);
            written(bench, &ask.to, &key, &ask.title, &ask.images, &t, cx)
        }
    };
    let standing = div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .children(lane.and_then(|lane| stand(ask, lane, &t)))
        .children(agents_on(bench, ask, handle, cx));
    let head = div()
        .px(SPACE_3H)
        .pt(SPACE_3)
        .pb(SPACE_2)
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(caps)
        .child(
            div()
                .text_size(theme::ui_text(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_primary)
                .child(named),
        )
        .child(standing);
    let said = sheet.said.iter().enumerate().map(|(at, said)| {
        let key = format!("ask-{}-said-{at}", ask.id);
        said_line(bench, ask, said, at == 0, &key, handle, &t, cx)
    });
    let body = div()
        .id(SharedString::from(format!("ask-sheet-{}", ask.id)))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(lab(t!("You wrote"), cx))
        .children(said)
        .when(!ask.pages.is_empty() || !ask.files.is_empty(), |body| {
            body.child(lab(t!("Opened on its desk"), cx))
                .children(links(bench, ask, handle, cx))
        });
    let foot = div().p(SPACE_3H).child(
        ui::Button::new("ask-sheet-chat", t!("Open chat"), ui::Variant::Outline)
            .wide()
            .on_click(chat),
    );
    let scrim = div()
        .id("ask-sheet-scrim")
        .absolute()
        .inset_0()
        .bg(t.surface_sunken.opacity(0.55))
        .clicks(on(Workbench::close_sheet));
    let panel = div()
        .id("ask-sheet")
        .absolute()
        .top(SPACE_3)
        .right(SPACE_3)
        .bottom(SPACE_3)
        .w(px(SHEET_W))
        .flex()
        .flex_col()
        .rounded(radius_3())
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .shadow(t.shadow_float())
        .overflow_hidden()
        .occlude()
        .child(head)
        .child(body)
        .child(foot);
    div()
        .absolute()
        .inset_0()
        .child(scrim)
        .child(panel)
        .into_any_element()
}

fn lab(words: &str, cx: &App) -> Div {
    div()
        .px(SPACE_3H)
        .pt(SPACE_3)
        .pb(SPACE_2)
        .child(ui::caps(words, cx))
}

/// The agents an ask moves with, each with its turn's time and mark.
fn agents_on(
    bench: &Workbench,
    ask: &NowAsk,
    handle: &WeakEntity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let AskStand::Working { agents } = &ask.stand else {
        return Vec::new();
    };
    (agents.iter())
        .filter_map(|id| bench.agent_by_id(id))
        .map(|agent| {
            let turn = agent.turn.as_ref().filter(|_| agent.reachable);
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(crate::chat::agent_pill(&agent, 12., handle, cx))
                .children(ui::turn_time(&agent.id, turn))
                .children(ui::turn_mark(&agent.id, turn))
                .into_any_element()
        })
        .collect()
}

/// One message the person sent under the ask: when, and whether it asked, followed up or
/// answered an agent's call, then what they wrote with the pictures they sent.
#[allow(clippy::too_many_arguments)]
fn said_line(
    bench: &Workbench,
    ask: &NowAsk,
    said: &AskSaid,
    first: bool,
    key: &str,
    handle: &WeakEntity<Workbench>,
    t: &Tokens,
    cx: &App,
) -> Div {
    let kind: AnyElement = match (&said.call, first) {
        (Some(call), _) => {
            let asker = (bench.agent_by_id(&call.asker))
                .map(|agent| crate::chat::agent_pill(&agent, 11., handle, cx).into_any_element());
            div()
                .flex()
                .items_center()
                .gap(SPACE_1)
                .child(t!("Answered"))
                .children(asker)
                .into_any_element()
        }
        (None, true) => div().child(t!("Asked")).into_any_element(),
        (None, false) => div().child(t!("Follow-up")).into_any_element(),
    };
    let when = div()
        .flex()
        .items_center()
        .justify_between()
        .text_size(theme::ui_text(11.))
        .text_color(t.text_tertiary)
        .child(kind)
        .child(
            div()
                .font_features(theme::tabular())
                .child(ui::time::clock(said.at)),
        );
    let question = said.call.as_ref().map(|call| {
        ui::text(call.question.clone(), TEXT_XS, FontWeight::NORMAL)
            .whitespace_normal()
            .text_color(t.text_tertiary)
    });
    let pictures = (!said.images.is_empty()).then(|| {
        div()
            .flex()
            .flex_wrap()
            .gap(SPACE_1H)
            .children(said.images.iter().map(|path| {
                let picture = Picture::File(path.into()).source_at(px(SAID_THUMB_W * 2.));
                div()
                    .w(px(SAID_THUMB_W))
                    .h(px(SAID_THUMB_H))
                    .rounded(radius_2())
                    .overflow_hidden()
                    .bg(t.surface_sunken)
                    .child(img(picture).size_full().object_fit(ObjectFit::Cover))
            }))
    });
    let card = div()
        .mx(SPACE_3)
        .mb(SPACE_2)
        .px(SPACE_3)
        .py(px(9.))
        .rounded(radius_3())
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .text_size(theme::ui_text(13.))
        .text_color(t.text_primary)
        .child(when)
        .children(question)
        .child(written(
            bench,
            &ask.to,
            key,
            &said.text,
            &said.images,
            t,
            cx,
        ))
        .children(pictures);
    match said.call.is_some() {
        true => card.border_1().border_color(t.border),
        false => card.bg(t.surface_raised),
    }
}
