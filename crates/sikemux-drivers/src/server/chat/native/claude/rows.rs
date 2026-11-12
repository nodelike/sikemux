//! Claude Code's own tool calls and transcripts, read as the rows a client draws.
//! `fixtures/claude_rows.json` holds each input with its rows. After changing what a
//! chat shows on purpose, write them again with `SIKEMUX_WRITE_ROWS=1` and read the diff.

use std::path::Path;

use serde_json::{json, Value};
use sikemux_fold::{ChatEventKind, Fold};

use super::{replay, tools};
use crate::launch::PermissionMode;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/server/chat/native/claude/fixtures/claude_rows.json"
);

const ROOT: &str = "session-1";

fn rows_of(updates: impl IntoIterator<Item = (String, Value)>) -> Value {
    let mut fold = Fold::new(PermissionMode::WorkspaceWrite);
    fold.apply(0, ChatEventKind::TurnStarted, &json!({}), 1_000);
    for (index, (session, update)) in updates.into_iter().enumerate() {
        fold.apply(
            index as u64 + 1,
            ChatEventKind::SessionUpdate,
            &json!({ "sessionId": session, "update": update }),
            1_000,
        );
    }
    fold.flush();
    let view = fold.view(false);
    json!({ "rows": view.rows, "subagents": view.subagents })
}

fn at_root(update: Value) -> (String, Value) {
    (ROOT.to_owned(), update)
}

fn read(fixture: &mut Value) {
    let cwd = fixture["cwd"].as_str().expect("cwd").to_owned();
    let cwd = Path::new(&cwd);
    for case in fixture["calls"].as_array_mut().expect("calls") {
        let name = case["name"].as_str().expect("name");
        case["shown"] = rows_of([
            at_root(tools::tool_call("toolu_1", name, &case["input"], cwd)),
            at_root(tools::tool_call_refined(
                "toolu_1",
                name,
                &case["input"],
                cwd,
            )),
        ]);
    }
    for case in fixture["results"].as_array_mut().expect("results") {
        let name = case["name"].as_str().expect("name");
        let tool_use_result = Some(&case["tool_use_result"]).filter(|value| !value.is_null());
        case["shown"] = rows_of([
            at_root(tools::tool_call("toolu_1", name, &case["input"], cwd)),
            at_root(tools::tool_result(
                "toolu_1",
                name,
                &case["input"],
                &case["result"],
                tool_use_result,
                cwd,
            )),
        ]);
    }
    for case in fixture["transcripts"].as_array_mut().expect("transcripts") {
        let session = case["session_id"].as_str().expect("session");
        let cwd = Path::new(case["cwd"].as_str().expect("cwd"));
        let records = case["records"].as_array().expect("records");
        let shown = rows_of(replay::replay(records, session, cwd));
        let model = replay::resumed_model(records);
        case["shown"] = shown;
        case["model"] = json!(model);
    }
}

#[test]
fn claudes_input_reads_as_the_rows_a_client_draws() {
    let kept: Value =
        serde_json::from_str(include_str!("fixtures/claude_rows.json")).expect("fixture");
    let mut read_now = kept.clone();
    read(&mut read_now);
    if std::env::var_os("SIKEMUX_WRITE_ROWS").is_some() {
        let written = serde_json::to_string_pretty(&read_now).expect("rows");
        std::fs::write(FIXTURE, written + "\n").expect("fixture written");
        return;
    }
    for group in ["calls", "results", "transcripts"] {
        let (now, then) = (&read_now[group], &kept[group]);
        for (index, case) in then.as_array().expect("cases").iter().enumerate() {
            assert_eq!(
                now[index]["shown"], case["shown"],
                "{group} {index}: {case}"
            );
            assert_eq!(now[index]["model"], case["model"], "{group} {index}");
        }
    }
}
