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
//! Version 6: environments became places, and the environments an earlier core kept for
//! folders each project under that folder now keeps as its own.

use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;

use crate::server::places::place::{from_environment, Place, StepPass};

/// The environments set for folders, by folder, in the file an earlier core kept them in.
pub fn folder_environments(file: &std::path::Path) -> Vec<(String, Vec<Value>)> {
    let Some(kept) = std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Map<String, Value>>(&text).ok())
    else {
        return Vec::new();
    };
    (kept.into_iter())
        .filter_map(|(folder, listed)| Some((folder, listed.as_array()?.clone())))
        .collect()
}

pub fn carry_over(mut board: Value, folders: Option<&Vec<(String, Vec<Value>)>>) -> Value {
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
    if version < 6 {
        environments_to_places(&mut board, folders.map_or(&[][..], Vec::as_slice));
    }
    board
}

/// Adds `place` to `places`, or joins it to the one of the same id, which takes the stricter
/// of the two: production if either is, and a step that waits on the check if either does.
fn stricter(places: &mut Vec<Place>, place: Place) {
    let Some(kept) = places
        .iter_mut()
        .find(|kept| kept.shown.id == place.shown.id)
    else {
        places.push(place);
        return;
    };
    kept.shown.production |= place.shown.production;
    if kept.shown.production {
        kept.shown.level = sikemux_view::places::Level::Mine;
    }
    kept.step = match (kept.step, place.step) {
        (_, Some(StepPass::Check)) | (Some(StepPass::Check), _) if !kept.shown.production => {
            Some(StepPass::Check)
        }
        (None, None) => None,
        _ => Some(StepPass::Deploy),
    };
    for fact in place.recognise {
        if !kept.recognise.contains(&fact) {
            kept.recognise.push(fact);
        }
    }
    kept.health = kept.health.take().or(place.health);
}

fn environments_to_places(board: &mut Value, folders: &[(String, Vec<Value>)]) {
    let project = board["project"].as_str().unwrap_or_default().to_owned();
    let inherited = (folders.iter())
        .filter(|(folder, _)| std::path::Path::new(&project).starts_with(folder))
        .flat_map(|(_, listed)| listed.iter());
    let Some(settings) = board.get_mut("settings").and_then(Value::as_object_mut) else {
        return;
    };
    let own = settings.remove("environments");
    let mut places: Vec<Place> = Vec::new();
    for environment in (own
        .iter()
        .flat_map(|own| own.as_array().into_iter().flatten()))
    .chain(inherited)
    {
        match from_environment(environment) {
            Some(place) => stricter(&mut places, place),
            None => eprintln!(
                "sikemux core: an environment of {project} was not carried over, as it named nothing to recognise it by: {environment}"
            ),
        }
    }
    if !places.is_empty() {
        board["environmentsToPlaces"] = json!(places);
    }
    let Some(board) = board.as_object_mut() else {
        return;
    };
    let Some(proposal) = board.remove("environmentsProposal") else {
        return;
    };
    let proposed: Vec<Place> = (proposal["environments"].as_array().into_iter().flatten())
        .filter_map(from_environment)
        .collect();
    if !proposed.is_empty() {
        board.insert(
            "placesProposal".into(),
            json!({ "places": proposed, "why": proposal["why"], "by": proposal["by"], "at": proposal["at"] }),
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two environments with one id joined into the first, whose being no production stood.
    #[test]
    fn of_two_environments_with_one_id_the_stricter_wins() {
        let board = json!({ "version": 5, "project": "/work/shop", "settings": { "environments": [
            { "name": "Staging", "passes": "nobody", "recognise": [{ "kind": "rundeckJob", "path": "staging/*" }] },
        ] } });
        let folders = vec![(
            "/work".to_owned(),
            vec![
                json!({ "name": "staging", "passes": "person", "recognise": [{ "kind": "rundeckJob", "path": "shared/*" }] }),
            ],
        )];
        let carried = carry_over(board, Some(&folders));
        let places: Vec<Place> =
            serde_json::from_value(carried["environmentsToPlaces"].clone()).unwrap();
        assert_eq!(places.len(), 1);
        assert!(places[0].shown.production);
        assert_eq!(places[0].recognise.len(), 2);
    }
}
