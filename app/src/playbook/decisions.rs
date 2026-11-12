//! Overruling what agents decided for the person, and putting it back, on the host whose chat
//! shows the decision.

use gpui_kit::Context;
use sikemux_core::server::board::goal::{OverruleParams, UndoOverruleParams};
use sikemux_core::server::call;
use sikemux_view::chat::{AwayDecision, OverruledDecision};

use crate::core::Target;
use crate::toast;
use crate::workbench::Workbench;

pub(crate) fn overrule(
    bench: &mut Workbench,
    target: &Target,
    decision: &AwayDecision,
    cx: &mut Context<Workbench>,
) {
    let asked = target.request_typed::<call::OverruleDecision>(OverruleParams {
        project: decision.project.clone(),
        goal: decision.goal,
        decision: decision.decision,
        instead: None,
    });
    bench.report(
        asked,
        t!("It was not overruled"),
        |bench, _, cx| bench.notify(toast::success(t!("Overruled")), cx),
        cx,
    );
}

pub(crate) fn undo_overrule(
    bench: &mut Workbench,
    target: &Target,
    overruled: OverruledDecision,
    cx: &mut Context<Workbench>,
) {
    let asked = target.request_typed::<call::UndoOverrule>(UndoOverruleParams {
        project: overruled.project,
        goal: overruled.goal,
        decision: overruled.decision,
    });
    bench.report(
        asked,
        t!("It was not put back"),
        |bench, _, cx| bench.notify(toast::success(t!("The decision stands again")), cx),
        cx,
    );
}
