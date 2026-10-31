//! Carries a board file an earlier core wrote over to how this one keeps it,
//! before it is read: Auto's runs once named in their stage that the person
//! was to review, that they were handed back, or that they were finished,
//! which the card's slice, its status and the run's `held` say now.

use serde_json::Value;

pub(super) fn carry_over(mut board: Value) -> Value {
    let version = board.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version < 1 {
        runs_off_their_stage(&mut board);
    }
    board
}

fn runs_off_their_stage(board: &mut Value) {
    let Some(cards) = board.get_mut("cards").and_then(Value::as_array_mut) else {
        return;
    };
    for card in cards {
        let stage = card
            .pointer("/auto/stage")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (stage, held, done) = match stage.as_deref() {
            Some("review") => ("watching", None, false),
            Some("held") => ("working", Some("fix"), false),
            Some("merged") => ("merging", None, true),
            Some("closed") => ("working", None, true),
            _ => continue,
        };
        card["auto"]["stage"] = stage.into();
        if let Some(held) = held {
            card["auto"]["held"] = held.into();
        }
        if done {
            card["status"] = "done".into();
        }
    }
}
