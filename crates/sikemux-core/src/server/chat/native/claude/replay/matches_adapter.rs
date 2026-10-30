//! Compares `replay` against claude-agent-acp 0.81.2's own `replaySessionHistory` run on the
//! synthetic records in `tests/fixtures/adapter_replay.json` (subagents, plans, failures).

use std::path::Path;

use super::super::replay;
use super::super::tools::matches_adapter::without_pictures;
use serde_json::{json, Value};

#[test]
fn replay_matches_adapter() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/adapter_replay.json")).expect("fixture");
    for case in fixture.as_array().expect("cases") {
        let session = case["session_id"].as_str().expect("session");
        let cwd = Path::new(case["cwd"].as_str().expect("cwd"));
        let records = case["records"].as_array().expect("records");
        let actual: Vec<Value> = replay::replay(records, session, cwd)
            .into_iter()
            .map(|(target, update)| json!([target, update]))
            .collect();
        let expected: Vec<Value> = case["expected"]
            .as_array()
            .expect("expected")
            .iter()
            .map(|theirs| json!([theirs[0], without_pictures(&theirs[1])]))
            .collect();
        // The adapter says how every subagent ended after the whole chat; we say it where each ended.
        let (our_ends, ours) = split_ends(actual);
        let (their_ends, theirs) = split_ends(expected);
        for (index, (ours, theirs)) in ours.iter().zip(&theirs).enumerate() {
            assert_eq!(ours, theirs, "update {index} of {session}");
        }
        assert_eq!(ours.len(), theirs.len(), "update count of {session}");
        assert_eq!(our_ends, their_ends, "subagent ends of {session}");
        assert_eq!(
            replay::resumed_model(records)
                .map(Value::from)
                .unwrap_or(Value::Null),
            case["model"],
            "model of {session}"
        );
    }
}

fn split_ends(updates: Vec<Value>) -> (Vec<String>, Vec<Value>) {
    let (ends, rest): (Vec<Value>, Vec<Value>) = updates
        .into_iter()
        .partition(|update| update[1]["sessionUpdate"] == "subagent_state_update");
    let mut ends: Vec<String> = ends.iter().map(Value::to_string).collect();
    ends.sort();
    (ends, rest)
}
