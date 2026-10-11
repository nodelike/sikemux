//! Compares `replay` against claude-agent-acp 0.81.2's own `replaySessionHistory` run on the
//! synthetic records in `tests/fixtures/adapter_replay.json` (subagents, plans, failures).

use std::path::Path;

use super::super::replay;
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
        let expected = case["expected"].as_array().expect("expected");
        for (index, (ours, theirs)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(ours, theirs, "update {index} of {session}");
        }
        assert_eq!(actual.len(), expected.len(), "update count of {session}");
        assert_eq!(
            replay::resumed_model(records)
                .map(Value::from)
                .unwrap_or(Value::Null),
            case["model"],
            "model of {session}"
        );
    }
}
