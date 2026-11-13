//! A DBML block in an agent's reply, drawn as a diagram of its tables, with its text a
//! click away; and the pieces of such a block a Mermaid block shares.

use crate::ui::Clicks;
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AnyView, App, AppContext, Div, ElementId, Entity, InteractiveElement,
    IntoElement, ParentElement, SharedString, Stateful, StatefulInteractiveElement,
    StyleRefinement, Styled, WeakEntity, Window,
};
use gpui_transcript::{Fence, Fences};

use tokio::sync::mpsc::UnboundedSender;

use super::{math, mermaid};
use crate::core::Ask;
use crate::diagram::{Diagram, Kept, KeptKey};
use crate::theme::{self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2H, SPACE_3H};
use crate::workbench::Workbench;

/// Draws the ```dbml, ```mermaid and math blocks of `agent`'s chat, the math at the chat's
/// text `scale`.
pub(super) fn fences(
    bench: WeakEntity<Workbench>,
    agent: String,
    asks: UnboundedSender<Ask>,
    running: Rc<Cell<bool>>,
    scale: Rc<Cell<f32>>,
) -> Fences {
    Fences {
        langs: Rc::from(["dbml", mermaid::LANG, sikemux_markdown::MATH]),
        draw: Rc::new(move |fence, window, cx| {
            if fence.lang.eq_ignore_ascii_case(mermaid::LANG) {
                mermaid::block(fence, running.get(), window, cx)
            } else if fence.lang.eq_ignore_ascii_case(sikemux_markdown::MATH) {
                math::block(fence, running.get(), scale.get(), window, cx)
            } else {
                block(fence, &bench, &agent, &asks, window, cx)
            }
        }),
    }
}

/// What a block keeps between frames: its diagram, the text it was drawn from, the text as the
/// chat last had it, and whether the person chose to see the text.
struct Block {
    diagram: Entity<Diagram>,
    code: SharedString,
    written: SharedString,
    source: bool,
}

/// How long a block's text must stay the same before its last line counts as finished, for a
/// block whose reply stops right after it.
const SETTLED: Duration = Duration::from_millis(400);

fn block(
    fence: Fence,
    bench: &WeakEntity<Workbench>,
    agent: &str,
    asks: &UnboundedSender<Ask>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let key = ElementId::NamedChild(Arc::new(fence.id.clone()), "dbml".into());
    let written = fence.code.clone();
    let block = window.use_keyed_state(key, cx, |_, cx| Block {
        diagram: cx.new(|cx| {
            let mut diagram = Diagram::embedded(&written);
            let key = KeptKey::ByText(format!("chat:{agent}"));
            diagram.kept_by(
                Kept {
                    asks: asks.clone(),
                    key,
                },
                cx,
            );
            diagram
        }),
        code: written.clone(),
        written: written.clone(),
        source: false,
    });
    let diagram = block.read(cx).diagram.clone();
    if block.read(cx).written != written {
        let code = finished_lines(&written);
        block.update(cx, |kept, _| kept.written = written.clone());
        if code != written {
            settle(&block, written.clone(), cx);
        }
        if block.read(cx).code != code {
            block.update(cx, |kept, _| kept.code = code.clone());
            diagram.update(cx, |diagram, cx| diagram.set_source(&code, cx));
        }
    }
    let empty = diagram.read(cx).is_empty();
    let source = block.read(cx).source || empty;
    let t = Tokens::get(cx).clone();
    let card = crate::reading::code_card(&t);
    let shows = |id: &'static str, label: &'static str, shows_source: bool| {
        let block = block.clone();
        choice(id, label, source == shows_source, &t, move |cx| {
            block.update(cx, |block, cx| {
                block.source = shows_source;
                cx.notify();
            })
        })
    };
    let open = {
        let (code, bench, agent) = (fence.code.clone(), bench.clone(), agent.to_owned());
        chip("dbml-open")
            .text_color(t.text_disabled)
            .hover(|open| open.bg(t.surface_hover).text_color(t.text_primary))
            .tooltip(crate::ui::tooltip(t!(
                "Open on the desk, with room to spread out"
            )))
            .clicks(move |_, _, cx| open_on_desk(&code, &bench, &agent, cx))
            .child(t!("Open on desk"))
    };
    let title = head(fence.icon, fence.lang, &t)
        .when(!empty, |title| {
            title
                .child(shows("dbml-diagram", t!("Diagram"), false))
                .child(shows("dbml-source", t!("Source"), true))
                .child(div().w(px(1.)).h(px(10.)).mx(SPACE_1).bg(t.line_soft))
                .child(open)
                .child(div().w(px(6.)))
        })
        .child(fence.copy);
    div()
        .id(ElementId::NamedChild(
            Arc::new(fence.id),
            "dbml-card".into(),
        ))
        .rounded(card.radius)
        .bg(card.ground)
        .overflow_hidden()
        .child(title)
        .map(|frame| match source {
            true => frame.child(fence.lines),
            false => {
                let height = diagram.read(cx).height();
                frame.child(
                    AnyView::from(diagram)
                        .cached(StyleRefinement::default().w_full().h(px(height))),
                )
            }
        })
        .into_any_element()
}

/// Draws the whole of `written` once it has stayed the block's text for a moment, so a line the
/// reply ends on is not left out for want of another change to the chat.
fn settle(block: &Entity<Block>, written: SharedString, cx: &mut App) {
    let block = block.downgrade();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SETTLED).await;
        let _ = block.update(cx, |kept, cx| {
            if kept.written == written && kept.code != written {
                kept.code = written.clone();
                kept.diagram
                    .update(cx, |diagram, cx| diagram.set_source(&written, cx));
                cx.notify();
            }
        });
    })
    .detach();
}

/// The block's text up to its last line break: while an agent writes a line, the diagram
/// waits for the whole of it rather than drawing it again with each word.
fn finished_lines(code: &SharedString) -> SharedString {
    match code.rfind('\n') {
        Some(end) if end + 1 < code.len() => SharedString::from(code[..=end].to_string()),
        _ => code.clone(),
    }
}

/// The top of a block drawn as a picture: its language, then whatever the caller adds, which
/// ends with the block's copy button.
pub(super) fn head(icon: Option<AnyElement>, lang: SharedString, t: &Tokens) -> Div {
    let name = match icon {
        Some(icon) => icon,
        None => div().truncate().child(lang).into_any_element(),
    };
    div()
        .mt(SPACE_1H)
        .h(px(22.))
        .pl(SPACE_3H)
        .pr(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_0)
        .text_size(px(10.))
        .text_color(t.text_disabled)
        .child(div().flex_1().min_w_0().flex().child(name))
}

/// One of the chips that choose between a block's picture and its text.
pub(super) fn choice(
    id: &'static str,
    label: &'static str,
    chosen: bool,
    t: &Tokens,
    pick: impl Fn(&mut App) + 'static,
) -> Stateful<Div> {
    chip(id)
        .text_color(if chosen {
            t.text_primary
        } else {
            t.text_disabled
        })
        .map(|chip| match chosen {
            true => chip.bg(t.surface_chosen),
            false => chip.hover(|chip| chip.bg(t.surface_hover)),
        })
        .clicks(move |_, _, cx| pick(cx))
        .child(label)
}

fn chip(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(SPACE_1H)
        .h(px(17.))
        .flex()
        .items_center()
        .rounded(theme::RADIUS_1)
        .cursor_pointer()
}

/// Writes the schema to a file of its own and opens it on the agent's desk, where it has
/// room to spread out and its text can be edited.
fn open_on_desk(code: &SharedString, bench: &WeakEntity<Workbench>, agent: &str, cx: &mut App) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    let folder = std::env::temp_dir().join("sikemux-schemas");
    let path = folder.join(format!("schema-{:08x}.dbml", hasher.finish() as u32));
    let written = std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(&path, &**code));
    if written.is_err() {
        return;
    }
    let _ = bench.update(cx, |bench, cx| {
        bench.open_file_on_desk(agent, path, None, None, cx);
        bench.parts.redraw_stage(cx);
    });
}
