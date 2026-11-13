//! Overruling what agents decided for the person, and putting it back, on the host whose chat
//! shows the decision.

use gpui_kit::Context;
use sikemux_calls::board::OverruleParams;
use sikemux_calls::board::UndoOverruleParams;
use sikemux_calls::call;
use sikemux_view::chat::OverruledDecision;

use crate::core::Target;
use crate::toast;
use crate::workbench::Workbench;

/// Overrules decision `decision` of `project`, kept on goal `goal` or on the project's own log.
pub(crate) fn overrule(
    bench: &mut Workbench,
    target: &Target,
    (project, goal, decision): (String, Option<u64>, u64),
    cx: &mut Context<Workbench>,
) {
    let asked = target.request_typed::<call::OverruleDecision>(OverruleParams {
        project,
        goal,
        decision,
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
