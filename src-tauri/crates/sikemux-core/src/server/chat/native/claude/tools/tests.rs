use std::path::Path;

use serde_json::{json, Value};

use super::*;

const CWD: &str = "/repo/app";

fn cwd() -> &'static Path {
    Path::new(CWD)
}

fn result(content: Value, is_error: bool) -> Value {
    json!({ "type": "tool_result", "tool_use_id": "t", "content": content, "is_error": is_error })
}

fn text_of(update: &Value) -> &str {
    update["content"][0]["content"]["text"]
        .as_str()
        .unwrap_or_default()
}

#[test]
fn read_title_shows_range_and_relative_path() {
    let info = tool_info(
        "Read",
        &json!({ "file_path": "/repo/app/src/a.rs", "offset": 10, "limit": 5 }),
        cwd(),
    );
    assert_eq!(info.title, "Read src/a.rs (10 - 14)");
    assert_eq!(info.kind, "read");
    assert_eq!(
        info.locations,
        vec![json!({ "path": "/repo/app/src/a.rs", "line": 10 })]
    );

    let outside = tool_info(
        "Read",
        &json!({ "file_path": "/etc/hosts", "offset": 3 }),
        cwd(),
    );
    assert_eq!(outside.title, "Read /etc/hosts (from line 3)");
}

#[test]
fn read_result_numbers_lines_from_the_structured_file() {
    let update = tool_result(
        "t",
        "Read",
        &json!({ "file_path": "/repo/app/a.md" }),
        &result(json!("raw view"), false),
        Some(&json!({ "type": "text", "file": { "content": "```\nx\n", "startLine": 4 } })),
        cwd(),
    );
    assert_eq!(text_of(&update), "```\n4\t```\n5\tx\n```");
    assert_eq!(update["rawOutput"], "raw view");
}

#[test]
fn bash_output_is_a_console_fence() {
    let update = tool_result(
        "t",
        "Bash",
        &json!({ "command": "ls" }),
        &result(json!("a\nb\n\n"), false),
        None,
        cwd(),
    );
    assert_eq!(update["status"], "completed");
    assert_eq!(text_of(&update), "```console\na\nb\n```");
}

#[test]
fn errors_are_fenced_whatever_the_tool() {
    let update = tool_result(
        "t",
        "Bash",
        &json!({ "command": "false" }),
        &result(json!("Exit code 1"), true),
        Some(&json!({ "stdout": "", "stderr": "nope" })),
        cwd(),
    );
    assert_eq!(update["status"], "failed");
    assert_eq!(text_of(&update), "```\nExit code 1\n```");
}

#[test]
fn edit_and_write_show_the_optimistic_diff_and_no_result_content() {
    let edit = tool_call(
        "t",
        "Edit",
        &json!({ "file_path": "/repo/app/a.rs", "old_string": "a", "new_string": "b" }),
        cwd(),
    );
    assert_eq!(edit["title"], "Edit a.rs");
    assert_eq!(
        edit["content"],
        json!([{ "type": "diff", "path": "/repo/app/a.rs", "oldText": "a", "newText": "b" }])
    );
    let write = tool_info(
        "Write",
        &json!({ "path": "/repo/app/b.rs", "file_text": "x" }),
        cwd(),
    );
    assert_eq!(write.title, "Write b.rs");
    assert_eq!(write.content[0]["oldText"], Value::Null);
    assert_eq!(write.content[0]["newText"], "x");

    let done = tool_result(
        "t",
        "Edit",
        &json!({}),
        &result(json!("ok"), false),
        None,
        cwd(),
    );
    assert!(done.get("content").is_none());
}

#[test]
fn grep_title_reads_like_a_command_line() {
    let info = tool_info(
        "Grep",
        &json!({ "pattern": "fn", "-i": true, "output_mode": "count", "glob": "*.rs", "path": "src" }),
        cwd(),
    );
    assert_eq!(info.title, "grep -i -c --include=\"*.rs\" \"fn\" src");
}

#[test]
fn web_search_lists_hits() {
    let update = tool_result(
        "t",
        "WebSearch",
        &json!({ "query": "q" }),
        &result(json!("dump"), false),
        Some(
            &json!({ "results": ["Intro", { "content": [{ "title": "T", "url": "https://u" }] }] }),
        ),
        cwd(),
    );
    assert_eq!(text_of(&update), "Intro\nT (https://u)");
}

#[test]
fn agent_output_loses_the_trailer() {
    let update = tool_result(
        "t",
        "Agent",
        &json!({ "description": "d" }),
        &result(
            json!("Report\nagentId: a-1 (use SendMessage)\n<usage>x</usage>"),
            false,
        ),
        None,
        cwd(),
    );
    assert_eq!(text_of(&update), "Report");
}

#[test]
fn exit_plan_mode_unwraps_the_raw_output() {
    let update = tool_result(
        "t",
        "ExitPlanMode",
        &json!({ "plan": "p" }),
        &result(json!("```\nNo thanks\n```"), false),
        None,
        cwd(),
    );
    assert_eq!(update["rawOutput"], "No thanks");
    assert_eq!(update["title"], "Exited Plan Mode");
    assert_eq!(
        tool_info("ExitPlanMode", &json!({}), cwd()).kind,
        "switch_mode"
    );
}

#[test]
fn skill_and_mcp_titles() {
    assert_eq!(
        tool_info("Skill", &json!({ "skill": "pdf" }), cwd()).title,
        "Load skill: pdf"
    );
    let mcp = tool_info("mcp__github__get_issue", &json!({}), cwd());
    assert_eq!(
        (mcp.title.as_str(), mcp.kind),
        ("mcp__github__get_issue", "other")
    );
}

#[test]
fn generic_content_converts_images_and_unknown_blocks() {
    let update = tool_result(
        "t",
        "mcp__x__shot",
        &json!({}),
        &result(
            json!([
                { "type": "image", "source": { "type": "base64", "data": "QQ==", "media_type": "image/png" } },
                { "type": "tool_reference", "tool_name": "Foo" },
            ]),
            false,
        ),
        None,
        cwd(),
    );
    assert_eq!(
        update["content"][0]["content"],
        json!({ "type": "image", "data": "QQ==", "mimeType": "image/png" })
    );
    assert_eq!(update["content"][1]["content"]["text"], "Tool: Foo");
}

#[test]
fn plan_tools_resolve_without_content() {
    assert!(is_plan_tool("TodoWrite") && is_plan_tool("TaskGet") && !is_plan_tool("Bash"));
    assert!(is_subagent_tool("Task") && !is_subagent_tool("TaskCreate"));
    let update = tool_result(
        "t",
        "TodoWrite",
        &json!({}),
        &result(json!("ok"), false),
        None,
        cwd(),
    );
    assert_eq!(
        update,
        json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "completed", "rawOutput": "ok" })
    );
}

#[test]
fn refined_call_has_no_status() {
    let update = tool_call_refined("t", "Bash", &json!({ "command": "ls" }), cwd());
    assert_eq!(update["sessionUpdate"], "tool_call_update");
    assert!(update.get("status").is_none() && update.get("name").is_none());
    assert_eq!(update["title"], "ls");
}

#[test]
fn todo_plan_prefers_the_active_form_in_progress() {
    let plan = todo_plan(&json!({ "todos": [
        { "content": "Test", "status": "in_progress", "activeForm": "Testing" },
        { "content": "Ship", "status": "pending", "activeForm": "Shipping" },
    ] }));
    assert_eq!(
        plan,
        Some(json!({ "sessionUpdate": "plan", "entries": [
            { "content": "Testing", "status": "in_progress", "priority": "medium" },
            { "content": "Ship", "status": "pending", "priority": "medium" },
        ] }))
    );
    assert_eq!(todo_plan(&json!({})), None);
}

#[test]
fn task_plan_follows_create_update_list_and_hooks() {
    let mut plan = TaskPlan::default();
    let created = plan.apply_result(
        "TaskCreate",
        &json!({ "subject": "Write tests", "activeForm": "Writing tests" }),
        &result(json!("Task #1 created successfully: Write tests"), false),
        None,
    );
    assert_eq!(created.unwrap()["entries"][0]["content"], "Write tests");
    let updated = plan
        .apply_result(
            "TaskUpdate",
            &json!({ "taskId": "1", "status": "in_progress" }),
            &result(json!("Updated task #1"), false),
            Some(&json!({ "success": true, "taskId": "1", "updatedFields": ["status"] })),
        )
        .unwrap();
    assert_eq!(
        updated["entries"][0],
        json!({ "content": "Writing tests", "status": "in_progress", "priority": "medium" })
    );
    let refused = plan.apply_result(
        "TaskUpdate",
        &json!({ "taskId": "9", "status": "completed" }),
        &result(json!("Task #9 not found"), false),
        None,
    );
    assert_eq!(refused, None);
    let listed = plan
        .apply_result(
            "TaskList",
            &json!({}),
            &result(
                json!("#1 [in_progress] Write tests (me)\n#2 [pending] Ship [blocked by #1]"),
                false,
            ),
            None,
        )
        .unwrap();
    assert_eq!(listed["entries"][1]["content"], "Ship");
    assert_eq!(listed["entries"][0]["content"], "Writing tests");
    assert_eq!(plan.plan()["entries"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        plan.apply_result("TaskGet", &json!({}), &result(json!("x"), false), None),
        None
    );
}

#[test]
fn content_chunks_carry_the_message_id() {
    assert_eq!(
        content_chunk(
            &json!({ "type": "thinking", "thinking": "hm" }),
            "assistant",
            Some("msg_1")
        ),
        Some(
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hm" }, "messageId": "msg_1" })
        )
    );
    assert_eq!(
        content_chunk(&json!({ "type": "text", "text": "hi" }), "user", None),
        Some(
            json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "hi" } })
        )
    );
    assert_eq!(
        content_chunk(
            &json!({ "type": "thinking", "thinking": "" }),
            "assistant",
            None
        ),
        None
    );
    assert_eq!(
        content_chunk(&json!({ "type": "tool_use" }), "assistant", None),
        None
    );
}

#[test]
fn markdown_escape_outgrows_inner_fences() {
    assert_eq!(markdown_escape("a"), "```\na\n```");
    assert_eq!(markdown_escape("````\nx\n"), "`````\n````\nx\n`````");
}
