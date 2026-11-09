//! Carries a board file an earlier core wrote over to how this one keeps it,
//! before it is read. Version 1: Auto's runs once named in their stage that
//! the person was to review or that they were handed back, which the run's
//! own stage and `held` say now; a run that was finished goes, leaving the
//! card as it is. Version 2: where Auto finished, merging or stopping at a
//! pull request, became who passes Landed, Sikemux or the person. Version 3
//! changes nothing here: `lease::clear_old_stalls` clears stalls once the core
//! knows which chats are open, which a file can't say. Version 4: Looks
//! right left every slice's ladder for one look at each goal, which counts
//! as already taken of every slice built so far but those that waited for it.
//! Version 5: a slice's notes became changes its owner ticks off; those
//! written before were handled as messages, so they count as ticked.

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

pub(super) fn carry_over(mut board: Value) -> Value {
    let version = board.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version < 1 {
        runs_off_their_stage(&mut board);
    }
    if version < 2 {
        finish_to_landing(&mut board);
    }
    if version < 4 {
        looks_right_to_goals(&mut board);
    }
    if version < 5 {
        notes_to_changes(&mut board);
    }
    board
}

fn notes_to_changes(board: &mut Value) {
    let Some(cards) = board.get_mut("cards").and_then(Value::as_array_mut) else {
        return;
    };
    for card in cards {
        let Some(slice) = card.get_mut("slice").and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(notes) = slice.get_mut("notes").and_then(Value::as_array_mut) else {
            continue;
        };
        let mut id = 0;
        for note in notes.iter_mut() {
            id += 1;
            note["id"] = json!(id);
            note["ticked"] = json!({ "by": "", "at": note["at"], "proof": "" });
        }
        slice.insert("nextNote".to_owned(), json!(id + 1));
    }
}

fn looks_right_to_goals(board: &mut Value) {
    let Some(cards) = board.get_mut("cards").and_then(Value::as_array_mut) else {
        return;
    };
    let is_looks_right = |step: &Value| step.as_str() == Some("looksRight");
    let mut waited = Vec::new();
    for card in cards.iter_mut() {
        let done = card["status"].as_str() == Some("done");
        let Some(slice) = card.get_mut("slice").and_then(Value::as_object_mut) else {
            continue;
        };
        let ladder = slice.get_mut("ladder").and_then(Value::as_array_mut);
        let had = ladder.is_some_and(|ladder| {
            let had = ladder.iter().any(is_looks_right);
            ladder.retain(|step| !is_looks_right(step));
            had
        });
        let passes = slice.get_mut("passes").and_then(Value::as_array_mut);
        let passed = passes.is_some_and(|passes| {
            let passed = passes.iter().any(|pass| is_looks_right(&pass["gate"]));
            passes.retain(|pass| !is_looks_right(&pass["gate"]));
            passed
        });
        if had && !passed && !done {
            waited.extend(card["id"].as_u64());
        }
    }
    let parent = |card: &Value| card["parent"].as_u64();
    let ids: Vec<(u64, Option<u64>)> = (cards.iter())
        .filter_map(|card| Some((card["id"].as_u64()?, parent(card))))
        .collect();
    let at = epoch_ms();
    for card in cards.iter_mut().filter(|card| parent(card).is_none()) {
        let Some(goal) = card["id"].as_u64() else {
            continue;
        };
        let covered: Vec<u64> = (ids.iter())
            .filter(|(id, parent)| *id == goal || *parent == Some(goal))
            .map(|(id, _)| *id)
            .filter(|id| !waited.contains(id))
            .collect();
        card["looked"] = json!({ "at": at, "covered": covered });
    }
}

fn finish_to_landing(board: &mut Value) {
    let Some(settings) = board.get_mut("settings").and_then(Value::as_object_mut) else {
        return;
    };
    let landing = match settings.remove("finish").as_ref().and_then(Value::as_str) {
        Some("pr") => "person",
        _ => "core",
    };
    settings.insert("landing".into(), landing.into());
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
