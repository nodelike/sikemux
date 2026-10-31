//! Carries a board file an earlier core wrote over to how this one keeps it,
//! before it is read: Auto's runs once named in their stage that the person
//! was to review or that they were handed back, which the run's own stage and
//! `held` say now. A run that was finished goes, leaving the card as it is.

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
        match stage.as_deref() {
            Some("review") => card["auto"]["stage"] = "watching".into(),
            Some("held") => {
                card["auto"]["stage"] = "working".into();
                card["auto"]["held"] = "fix".into();
            }
            Some("merged" | "closed") => {
                if let Some(card) = card.as_object_mut() {
                    card.remove("auto");
                }
            }
            _ => {}
        }
    }
}
