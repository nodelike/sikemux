use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use super::*;

const SESSION: &str = "0b9cc1a6-6d0b-4c1e-9d55-0d2f6c5e7a10";

fn user(uuid: &str, parent: Option<&str>, content: Value) -> Value {
    json!({ "type": "user", "uuid": uuid, "parentUuid": parent, "message": { "role": "user", "content": content } })
}

fn assistant(uuid: &str, parent: Option<&str>, id: &str, model: &str, content: Value) -> Value {
    json!({
        "type": "assistant",
        "uuid": uuid,
        "parentUuid": parent,
        "message": { "id": id, "role": "assistant", "model": model, "content": content },
    })
}

fn write_transcript(dir: &Path, records: &[Value]) -> std::path::PathBuf {
    let project = dir.join("projects").join("-repo-app");
    std::fs::create_dir_all(&project).expect("project dir");
    let path = project.join(format!("{SESSION}.jsonl"));
    let mut file = std::fs::File::create(&path).expect("create");
    for record in records {
        writeln!(file, "{record}").expect("write");
    }
    path
}

#[test]
fn transcript_path_searches_every_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(transcript_path(dir.path(), SESSION), None);
    let path = write_transcript(dir.path(), &[user("u1", None, json!("hi"))]);
    std::fs::create_dir_all(dir.path().join("projects/-other")).expect("other dir");
    assert_eq!(transcript_path(dir.path(), SESSION), Some(path));
    assert_eq!(transcript_path(dir.path(), "../../etc/passwd"), None);

    let empty = dir
        .path()
        .join("projects/-other/a3d1c6a2-0000-4000-8000-000000000000.jsonl");
    std::fs::File::create(&empty).expect("empty file");
    assert_eq!(
        transcript_path(dir.path(), "a3d1c6a2-0000-4000-8000-000000000000"),
        None
    );
}

#[test]
fn read_chain_follows_the_latest_branch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_transcript(
        dir.path(),
        &[
            json!({ "type": "queue-operation" }),
            user("u1", None, json!("first")),
            assistant(
                "a1",
                Some("u1"),
                "m1",
                "claude-opus-5-5",
                json!([{ "type": "text", "text": "one" }]),
            ),
            user("u2", Some("a1"), json!("second")),
            assistant(
                "a2",
                Some("u2"),
                "m2",
                "claude-opus-5-5",
                json!([{ "type": "text", "text": "two" }]),
            ),
            user("u3", Some("a1"), json!("second, edited")),
            json!({ "type": "attachment", "uuid": "x", "parentUuid": "u3", "attachment": { "type": "date" } }),
            assistant(
                "a3",
                Some("x"),
                "m3",
                "claude-opus-5-5",
                json!([{ "type": "text", "text": "three" }]),
            ),
        ],
    );
    let chain = read_chain(&path).expect("chain");
    let uuids: Vec<&str> = chain.iter().filter_map(|r| r["uuid"].as_str()).collect();
    assert_eq!(uuids, ["u1", "a1", "u3", "a3"]);
    assert_eq!(rewind_point(&chain, "u3"), Some("a1".to_string()));
    assert_eq!(rewind_point(&chain, "u1"), None);
    assert_eq!(rewind_point(&chain, "missing"), None);
    assert!(read_chain(&dir.path().join("nope.jsonl")).is_err());
}

#[test]
fn rewind_point_skips_records_the_chain_dropped() {
    let records = [
        user("u1", None, json!("first")),
        assistant("a1", Some("u1"), "m1", "m", json!("one")),
        user("u2", Some("dropped-attachment"), json!("second")),
    ];
    assert_eq!(rewind_point(&records, "u2"), Some("a1".to_string()));
}

#[test]
fn resumed_model_skips_synthetic_and_subagent_messages() {
    let records = [
        assistant("a1", None, "m1", "claude-opus-5-5", json!("x")),
        json!({ "type": "assistant", "uuid": "s", "parent_tool_use_id": "toolu_1",
                "message": { "model": "claude-haiku-4-5", "content": "y" } }),
        assistant("a2", Some("a1"), "m2", "<synthetic>", json!("limit")),
    ];
    assert_eq!(resumed_model(&records), Some("claude-opus-5-5".to_string()));
    assert_eq!(resumed_model(&records[2..]), None);
}

#[test]
fn local_command_markers_are_stripped() {
    assert_eq!(
        strip_local_command_metadata(&json!("hi<system-reminder>secret</system-reminder>")),
        Some(json!("hi"))
    );
    assert_eq!(
        strip_local_command_metadata(&json!(
            "<command-name>/fix</command-name><command-args> now </command-args>"
        )),
        Some(json!("/fix now"))
    );
    assert_eq!(
        strip_local_command_metadata(&json!("<command-name>/compact</command-name>")),
        None
    );
    assert_eq!(
        strip_local_command_metadata(&json!("<local-command-stdout>x</local-command-stdout>")),
        None
    );
    assert_eq!(
        strip_local_command_metadata(
            &json!([{ "type": "text", "text": "<system-reminder>r</system-reminder>" }])
        ),
        None
    );
    assert_eq!(
        strip_local_command_metadata(&json!("a <system-reminder> unclosed")),
        Some(json!("a <system-reminder> unclosed"))
    );
}

#[test]
fn replay_emits_prompts_replies_and_tools() {
    let cwd = Path::new("/repo/app");
    let records = [
        user(
            "u1",
            None,
            json!("Fix it<system-reminder>r</system-reminder>"),
        ),
        assistant(
            "a1",
            Some("u1"),
            "msg_1",
            "claude-opus-5-5",
            json!([{ "type": "text", "text": "On it" }]),
        ),
        assistant(
            "a2",
            Some("a1"),
            "msg_1",
            "claude-opus-5-5",
            json!([
                { "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": { "command": "make" } },
                { "type": "tool_use", "id": "toolu_2", "name": "Agent", "input": { "description": "d" } },
            ]),
        ),
        user(
            "u2",
            Some("a2"),
            json!([
                { "type": "tool_result", "tool_use_id": "toolu_1", "content": "built" },
                { "type": "tool_result", "tool_use_id": "toolu_2", "content": "report" },
            ]),
        ),
        assistant(
            "a3",
            Some("u2"),
            "msg_2",
            "<synthetic>",
            json!([{ "type": "text", "text": "Please run /login" }]),
        ),
    ];
    let updates = replay(&records, SESSION, cwd);
    let kinds: Vec<(&str, &str)> = updates
        .iter()
        .map(|(session, update)| {
            (
                session.as_str(),
                update["sessionUpdate"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (SESSION, "user_message_chunk"),
            (SESSION, "agent_message_chunk"),
            (SESSION, "tool_call"),
            (SESSION, "tool_call_update"),
        ]
    );
    assert_eq!(updates[0].1["content"]["text"], "Fix it");
    assert_eq!(updates[0].1["messageId"], "u1");
    assert_eq!(updates[1].1["messageId"], "msg_1");
    assert_eq!(
        updates[3].1["content"][0]["content"]["text"],
        "```console\nbuilt\n```"
    );
}

#[test]
fn replay_routes_subagent_records_to_child_sessions() {
    let records = [
        assistant(
            "a1",
            None,
            "msg_1",
            "m",
            json!([
                { "type": "tool_use", "id": "toolu_a", "name": "Task", "input": { "description": "Scout", "prompt": "Look" } },
            ]),
        ),
        json!({ "type": "assistant", "uuid": "s1", "parent_tool_use_id": "toolu_a",
                "message": { "id": "msg_s", "role": "assistant", "content": [{ "type": "text", "text": "inside" }] } }),
        user(
            "u1",
            Some("a1"),
            json!([{ "type": "tool_result", "tool_use_id": "toolu_a", "content": "Agent stopped", "is_error": true }]),
        ),
    ];
    let child = format!("{SESSION}:replay-subagent:toolu_a");
    let updates = replay(&records, SESSION, Path::new("/repo/app"));
    assert_eq!(
        updates,
        vec![
            (
                SESSION.to_string(),
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": child,
                                           "name": "Scout", "task": "Look", "capabilities": {} })
            ),
            (
                child.clone(),
                json!({ "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text", "text": "inside" }, "messageId": "msg_s" })
            ),
            (
                SESSION.to_string(),
                json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": child,
                                           "state": "cancelled" })
            ),
        ]
    );
}

#[test]
fn a_subagents_own_transcript_replays_inside_its_child_session() {
    let dir = tempfile::tempdir().expect("dir");
    let path = write_transcript(
        dir.path(),
        &[
            user(
                "u1",
                None,
                json!([{ "type": "text", "text": "look around" }]),
            ),
            assistant(
                "a1",
                Some("u1"),
                "msg_1",
                "claude-opus-5-5",
                json!([{ "type": "tool_use", "id": "toolu_agent", "name": "Agent",
                         "input": { "description": "Explore the repo", "prompt": "Find the tests" } }]),
            ),
            user(
                "r1",
                Some("a1"),
                json!([{ "type": "tool_result", "tool_use_id": "toolu_agent", "content": "Found them" }]),
            ),
            assistant(
                "a2",
                Some("r1"),
                "msg_2",
                "claude-opus-5-5",
                json!([{ "type": "text", "text": "Done" }]),
            ),
        ],
    );
    let agents = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&agents).expect("agents dir");
    std::fs::write(
        agents.join("agent-x.meta.json"),
        json!({ "agentType": "Explore", "toolUseId": "toolu_agent" }).to_string(),
    )
    .expect("meta");
    let mut side = user("s1", None, json!("Find the tests"));
    side["isSidechain"] = json!(true);
    let mut read = assistant(
        "s2",
        Some("s1"),
        "msg_s",
        "claude-haiku-4-5",
        json!([{ "type": "tool_use", "id": "toolu_read", "name": "Read", "input": { "file_path": "/repo/app/a.rs" } }]),
    );
    read["isSidechain"] = json!(true);
    std::fs::write(agents.join("agent-x.jsonl"), format!("{side}\n{read}\n")).expect("agent");

    let records = with_subagents(&path, read_chain(&path).expect("chain"));
    let updates = replay(&records, SESSION, Path::new("/repo/app"));
    let child = format!("{SESSION}:replay-subagent:toolu_agent");
    let spawned = updates
        .iter()
        .position(|(_, update)| update["sessionUpdate"] == "subagent_spawned")
        .expect("the subagent is announced");
    assert_eq!(updates[spawned].1["subagentSessionId"], child.as_str());
    assert_eq!(updates[spawned].1["name"], "Explore the repo");
    let inside: Vec<&Value> = updates
        .iter()
        .filter(|(session, _)| *session == child)
        .map(|(_, update)| update)
        .collect();
    assert_eq!(inside.len(), 1, "{inside:?}");
    assert_eq!(inside[0]["title"], "Read a.rs");
    assert!(updates.iter().any(|(session, update)| *session == SESSION
        && update["sessionUpdate"] == "subagent_state_update"
        && update["state"] == "completed"));
    assert_eq!(resumed_model(&records).as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn a_transcript_read_in_pieces_keeps_its_order() {
    let line = |n: usize| {
        json!({ "type": "user", "uuid": format!("u{n}"), "message": { "content": "x".repeat(2048) } })
            .to_string()
    };
    let text: String = (0..(PIECE_BYTES * 3 / 2048))
        .map(|n| line(n) + "\n")
        .collect();
    let entries = parse_entries(text.as_bytes());
    assert_eq!(entries.len(), PIECE_BYTES * 3 / 2048);
    assert!(entries
        .iter()
        .enumerate()
        .all(|(n, entry)| entry["uuid"] == format!("u{n}")));
}
