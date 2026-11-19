//! Overruling what agents decided for the person, and putting it back, on the host whose chat
//! shows the decision. A card the board closed by itself is put back the same way.

use gpui_kit::Context;
use sikemux_calls::board::{BoardUpdateParams, CardStatus, OverruleParams, UndoOverruleParams};
use sikemux_calls::call;
use sikemux_view::chat::{ClosedCard, OverruledDecision};

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

/// Reopens a card the board closed by itself, back to being worked.
pub(crate) fn reopen_card(
    bench: &mut Workbench,
    target: &Target,
    closed: ClosedCard,
    cx: &mut Context<Workbench>,
) {
    let asked = target.request_typed::<call::BoardUpdate>(BoardUpdateParams {
        project: closed.project,
        id: closed.card,
        title: None,
        details: None,
        acceptance: None,
        promise: None,
        asserts: None,
        status: Some(CardStatus::Progress),
        result: None,
        depends_on: None,
        parent: None,
        undrop: false,
        opened: false,
    });
    bench.report(
        asked,
        t!("It was not reopened"),
        |bench, _, cx| bench.notify(toast::success(t!("Reopened")), cx),
        cx,
    );
}
