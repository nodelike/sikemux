//! Compares `read_chain` with the Claude Agent SDK's `getSessionMessages` on the synthetic
//! transcripts in `tests/fixtures/sdk_chains.json` (forks, compaction, parallel tool calls,
//! queued prompts, local commands).

use std::io::Write;

use super::super::replay;
use serde_json::{json, Value};

#[test]
fn read_chain_matches_sdk() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/sdk_chains.json")).expect("fixture");
    for (name, case) in fixture.as_object().expect("cases") {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        for record in case["records"].as_array().expect("records") {
            writeln!(file, "{record}").expect("write");
        }
        let chain = replay::read_chain(file.path()).expect("read");
        let actual: Vec<Value> = chain
            .iter()
            .map(|r| json!({ "uuid": r["uuid"], "type": r["type"], "message": r["message"] }))
            .collect();
        assert_eq!(Value::Array(actual), case["expected"], "chain of {name}");
    }
}
