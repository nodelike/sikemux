//! The line a chat leaves where it went on with another model or agent, handed what it said.

use super::*;
use sikemux_view::chat::{ContextHandoff, HandoffSide};

use crate::theme::{SPACE_1, SPACE_3};

pub(super) fn line(handoff: &ContextHandoff, looks: &Drawn, cx: &App) -> AnyElement {
    let rule = || div().flex_1().min_w(px(12.)).h(px(1.)).bg(looks.rule);
    notice_box(looks)
        .pr(SPACE_3)
        .child(rule())
        .child(ui::icon(Icon::Swap, 12.).text_color(looks.quiet))
        .child(div().flex_none().child(t!("Context handoff")))
        .child(side(&handoff.from, looks, cx))
        .child(div().flex_none().child("→"))
        .child(side(&handoff.to, looks, cx))
        .child(rule())
        .into_any_element()
}

fn side(side: &HandoffSide, looks: &Drawn, cx: &App) -> Div {
    let name = match &side.model {
        Some(model) => model.clone(),
        None => (looks.bench.upgrade())
            .map(|bench| bench.read(cx).agent_name(&side.agent).to_owned())
            .unwrap_or_else(|| side.agent.clone()),
    };
    let mark = match &side.maker {
        Some(maker) => crate::icons::model_provider(maker),
        None => crate::icons::agent(&side.agent),
    };
    let tint = maker_colour(side, cx);
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(SPACE_1)
        .child(ui::icon(mark, 12.).text_color(tint))
        .child(div().min_w_0().truncate().text_color(tint).child(name))
}

/// The brand of the agent its maker makes, else of the agent it ran on.
fn maker_colour(side: &HandoffSide, cx: &App) -> Hsla {
    let made =
        (side.maker.as_deref()).and_then(|maker| crate::reads::Catalog::global(cx).by_maker(maker));
    match made {
        Some(agent) => theme::color(&agent.brand),
        None => theme::brand(&side.agent, cx),
    }
}
