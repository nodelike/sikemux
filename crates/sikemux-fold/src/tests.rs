use serde_json::json;
use sikemux_wire::chat::{
    ChatChange, ChatView, DiffLine, DiffSummary, OutputImage, OutputSummary, PageRef, PlanEntry,
    PlanPriority, PlanStatus, RowMeta, StopReason, ToolCall, ToolOutput, ToolStatus,
};

use super::*;

const ROOT: &str = "session-1";

struct Chat {
    fold: Fold,
    index: u64,
    at: u64,
}

impl Chat {
    fn new() -> Self {
        Self {
            fold: Fold::new(PermissionMode::WorkspaceWrite),
            index: 0,
            at: 1_000,
        }
    }

    fn running() -> Self {
        let mut chat = Self::new();
        chat.turn_started();
        chat
    }

    fn at(&mut self, at: u64) -> &mut Self {
        self.at = at;
        self
    }

    fn event(&mut self, kind: ChatEventKind, payload: Value) -> &mut Self {
        self.fold.apply(self.index, kind, &payload, self.at);
        self.index += 1;
        self
    }

    fn update(&mut self, update: Value) -> &mut Self {
        self.update_in(ROOT, update)
    }

    fn update_in(&mut self, session: &str, update: Value) -> &mut Self {
        self.event(
            ChatEventKind::SessionUpdate,
            json!({ "sessionId": session, "update": update }),
        )
    }

    fn prompt(&mut self, payload: Value) -> &mut Self {
        self.event(ChatEventKind::Prompt, payload)
    }

    fn turn_started(&mut self) -> &mut Self {
        self.event(ChatEventKind::TurnStarted, json!({}))
    }

    fn turn_completed(&mut self, stop_reason: Option<&str>) -> &mut Self {
        self.event(
            ChatEventKind::TurnCompleted,
            json!({ "stopReason": stop_reason }),
        )
    }

    fn status(&mut self, state: &str) -> &mut Self {
        self.event(ChatEventKind::Status, json!({ "state": state }))
    }

    fn ready(&mut self) -> &mut Self {
        self.event(
            ChatEventKind::Ready,
            json!({ "capabilities": {}, "setup": {} }),
        )
    }

    fn error(&mut self, payload: Value) -> &mut Self {
        self.event(ChatEventKind::Error, payload)
    }

    fn flush(&mut self) -> Vec<ChatChange> {
        self.fold.flush()
    }

    fn view(&mut self) -> ChatView {
        self.flush();
        self.fold.view(false)
    }

    fn rows(&mut self) -> Vec<ChatMessage> {
        self.view().rows
    }

    fn head(&self) -> &ChatHead {
        &self.fold.head
    }

    /// Whether `change` left the chat as it was.
    fn untouched_by(&mut self, change: impl FnOnce(&mut Self)) -> bool {
        self.flush();
        change(self);
        !self.fold.is_dirty()
    }

    /// Whether `change` left every row as it was.
    fn rows_untouched_by(&mut self, change: impl FnOnce(&mut Self)) -> bool {
        self.flush();
        change(self);
        self.flush()
            .iter()
            .all(|change| matches!(change, ChatChange::Head { .. }))
    }
}

fn say(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

fn chunk(text: &str, message: &str) -> Value {
    json!({ "sessionUpdate": "agent_message_chunk", "messageId": message, "content": { "type": "text", "text": text } })
}

fn text(id: &str, text: &str) -> ChatPart {
    ChatPart::Text {
        id: id.into(),
        text: text.into(),
        md: None,
        cut_bytes: None,
    }
}

fn plain(parts: &[ChatPart]) -> Vec<ChatPart> {
    parts
        .iter()
        .cloned()
        .map(|mut part| {
            if let ChatPart::Text { md, .. } | ChatPart::Thought { md, .. } = &mut part {
                *md = None;
            }
            part
        })
        .collect()
}

#[derive(Debug)]
struct Tool {
    tool: ToolCall,
    diff: Option<DiffSummary>,
    output: Option<OutputSummary>,
    page: Option<PageRef>,
    failure: Option<String>,
    started_at: Option<u64>,
    ended_at: Option<u64>,
}

fn tool_of(part: &ChatPart) -> Tool {
    match part {
        ChatPart::Tool {
            tool,
            diff,
            output,
            page,
            failure,
            started_at,
            ended_at,
            ..
        } => Tool {
            tool: tool.clone(),
            diff: diff.clone(),
            output: output.clone(),
            page: page.clone(),
            failure: failure.clone(),
            started_at: *started_at,
            ended_at: *ended_at,
        },
        other => panic!("expected a tool part, got {other:?}"),
    }
}

fn first_tool(chat: &mut Chat) -> Tool {
    tool_of(&chat.rows()[0].parts[0])
}

fn card_of(part: &ChatPart) -> SubagentCard {
    match part {
        ChatPart::Subagent { subagent, .. } => subagent.clone(),
        other => panic!("expected a subagent card, got {other:?}"),
    }
}

fn record(value: Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

mod reducer {
    use super::*;

    #[test]
    fn keeps_a_network_failure_with_what_the_agent_said() {
        let mut chat = Chat::new();
        chat.error(json!({
            "message": "Lost the connection to the model provider",
            "failure": { "kind": "network", "account": null, "details": "API Error: Connection error." },
        }));
        let failure = chat.head().failure.clone().expect("a failure");
        assert_eq!(failure.kind, FailureKind::Network);
        assert_eq!(
            failure.details.as_deref(),
            Some("API Error: Connection error.")
        );
        assert_eq!(failure.account, None);
    }

    fn retry(failure: Value) -> Value {
        let mut said = json!({ "id": "t:retry", "category": "connection", "title": "x" });
        for (key, value) in failure.as_object().expect("an object") {
            said[key] = value.clone();
        }
        json!({
            "sessionUpdate": "session_info_update",
            "_meta": { "jetbrains": { "air": { "version": 1, "sessionFailure": said } } },
        })
    }

    #[test]
    fn holds_the_agents_retries_until_it_streams_again_or_the_turn_ends() {
        let mut idle = Chat::new();
        idle.update(retry(json!({ "severity": "warning", "attempt": 1 })));
        assert_eq!(idle.head().retrying, None);

        let retrying_with = |failure: Value| {
            let mut chat = Chat::running();
            chat.update(retry(failure));
            chat
        };
        let attempt = |attempt: Option<u64>, connection: bool| {
            Some(Retrying {
                attempt,
                connection,
            })
        };
        assert_eq!(
            retrying_with(json!({ "severity": "warning", "attempt": 2 }))
                .head()
                .retrying,
            attempt(Some(2), true)
        );
        assert_eq!(
            retrying_with(json!({ "severity": "warning" }))
                .head()
                .retrying,
            attempt(None, true)
        );
        assert_eq!(
            retrying_with(json!({ "severity": "error", "attempt": 2 }))
                .head()
                .retrying,
            None
        );
        assert_eq!(
            retrying_with(json!({ "severity": "warning", "attempt": 1, "category": "service" }))
                .head()
                .retrying,
            attempt(Some(1), false)
        );

        let retrying = || retrying_with(json!({ "severity": "warning", "attempt": 2 }));
        let mut titled = retrying();
        titled.update(json!({ "sessionUpdate": "session_info_update", "title": "Fix login" }));
        assert_eq!(titled.head().retrying, attempt(Some(2), true));

        let mut streaming = retrying();
        streaming.update(say("agent_message_chunk", "Back"));
        assert_eq!(streaming.head().retrying, None);
        let mut completed = retrying();
        completed.turn_completed(None);
        assert_eq!(completed.head().retrying, None);
        let mut failed = retrying();
        failed.error(json!({ "message": "Connection error" }));
        assert_eq!(failed.head().retrying, None);
    }

    #[test]
    fn rebuilds_from_a_replay_with_streamed_text_joined_the_transcript_a_watching_client_built() {
        let tool = json!({ "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "Read file", "status": "in_progress" });
        let tool_done = json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed" });

        let mut watched = Chat::new();
        watched
            .update(say("user_message_chunk", "Look"))
            .turn_started();
        for (kind, piece) in [
            ("agent_thought_chunk", "Hm"),
            ("agent_thought_chunk", "m."),
            ("agent_message_chunk", "Rea"),
            ("agent_message_chunk", "ding."),
        ] {
            watched.update(say(kind, piece));
        }
        watched.update(tool.clone()).update(tool_done.clone());
        watched
            .update(say("agent_message_chunk", "Do"))
            .update(say("agent_message_chunk", "ne."))
            .turn_completed(Some("end_turn"));

        let mut replayed = Chat::new();
        replayed
            .update(say("user_message_chunk", "Look"))
            .turn_started()
            .update(say("agent_thought_chunk", "Hmm."))
            .update(say("agent_message_chunk", "Reading."))
            .update(tool)
            .update(tool_done)
            .update(say("agent_message_chunk", "Done."))
            .turn_completed(Some("end_turn"));

        assert_eq!(replayed.rows(), watched.rows());
        assert!(!replayed.head().running);
    }

    #[test]
    fn keeps_the_context_window_the_agent_reports_and_ignores_a_malformed_one() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "usage_update",
            "used": 84_000,
            "size": 200_000,
            "cost": { "amount": 1.25, "currency": "USD" },
        }));
        assert_eq!(
            chat.head().usage,
            Some(ContextUsage {
                used: 84_000.0,
                size: 200_000.0,
                cost: Some(UsageCost {
                    amount: 1.25,
                    currency: "USD".into()
                }),
            })
        );

        chat.update(json!({ "sessionUpdate": "usage_update", "used": 90_000, "size": 272_000 }));
        let codex = Some(ContextUsage {
            used: 90_000.0,
            size: 272_000.0,
            cost: None,
        });
        assert_eq!(chat.head().usage, codex);

        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "usage_update", "used": 1, "size": 0 }));
        }));
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "usage_update", "used": "lots", "size": 10 }));
        }));
        assert_eq!(chat.head().usage, codex);
    }

    #[test]
    fn merges_streamed_text_chunks_by_acp_message_id() {
        let mut chat = Chat::new();
        chat.update(chunk("Hello", "message-1"))
            .update(chunk(" world", "message-1"));
        let rows = chat.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(plain(&rows[0].parts), vec![text("m0.0", "Hello world")]);
    }

    #[test]
    fn keeps_consecutive_chunks_without_message_ids_in_one_message() {
        let mut chat = Chat::new();
        chat.update(say("agent_message_chunk", "one"))
            .update(say("agent_message_chunk", " two"));
        let rows = chat.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(plain(&rows[0].parts), vec![text("m0.0", "one two")]);
    }

    #[test]
    fn suppresses_the_agents_echo_of_a_prompt_a_person_sent() {
        let mut chat = Chat::new();
        chat.prompt(json!({ "text": "Check this", "paths": ["/tmp/example.ts"] }))
            .update(json!({
                "sessionUpdate": "user_message_chunk",
                "messageId": "remote-user-1",
                "content": { "type": "text", "text": "Check this" },
            }))
            .update(chunk("Done", "remote-agent-1"));
        let rows = chat.rows();
        assert_eq!(
            rows.iter().map(|row| row.role).collect::<Vec<_>>(),
            vec![ChatRole::User, ChatRole::Assistant]
        );
        assert_eq!(rows[0].attachments, vec!["/tmp/example.ts".to_owned()]);
    }

    #[test]
    fn merges_tool_updates_into_their_original_call() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Read file", "kind": "read", "status": "pending" }))
            .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed", "rawOutput": { "bytes": 42 } }));
        let part = first_tool(&mut chat);
        assert_eq!(part.tool.tool_call_id, "tool-1");
        assert_eq!(part.tool.title, "Read file");
        assert_eq!(part.tool.status, Some(ToolStatus::Completed));
        assert_eq!(part.output, None);
    }

    #[test]
    fn reads_a_finished_call_once_and_keeps_only_what_the_transcript_shows() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-1",
            "title": "Edit file",
            "kind": "edit",
            "status": "in_progress",
            "rawInput": { "file_path": "/repo/notes.md", "old_string": "one\n", "new_string": "two\n" },
        }))
        .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed" }));
        let part = first_tool(&mut chat);
        assert_eq!(
            part.diff,
            Some(DiffSummary {
                path: "/repo/notes.md".into(),
                adds: 1,
                dels: 1
            })
        );
        assert_eq!(
            part.tool.raw_input,
            Some(
                json!({ "file_path": "/repo/notes.md", "old_string": "one", "new_string": "two" })
            )
        );
        let detail = chat.fold.tool_detail("tool-1").expect("the change it made");
        assert_eq!(detail.diff.map(|diff| diff.lines.len()), Some(2));
    }

    #[test]
    fn keeps_only_short_arguments_of_a_finished_call() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-1",
            "title": "mcp__sikemux-tools__browser_click",
            "status": "in_progress",
            "rawInput": { "text": "Sign in", "role": "button", "report": "full", "steps": [{ "action": "click" }] },
        }))
        .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed" }));
        let kept = first_tool(&mut chat).tool.raw_input.expect("its arguments");
        assert_eq!(kept["text"], json!("Sign in"));
        assert_eq!(kept["role"], json!("button"));
        assert_eq!(kept["steps"], json!(1));
    }

    #[test]
    fn keeps_the_first_line_of_another_servers_arguments() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-1",
            "title": "mcp__linear__create_issue",
            "status": "in_progress",
            "rawInput": { "title": "Rows use underscores", "body": "long\nbody" },
        }))
        .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed" }));
        assert_eq!(
            first_tool(&mut chat).tool.raw_input,
            Some(json!({ "title": "Rows use underscores", "body": "long" }))
        );
    }

    #[test]
    fn keeps_a_failed_calls_message_and_drops_the_output_it_came_from() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-2", "title": "Write notes.md", "kind": "edit", "status": "in_progress" }))
            .update(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "tool-2",
                "status": "failed",
                "rawOutput": { "stderr": "  permission denied  " },
            }));
        let part = first_tool(&mut chat);
        assert_eq!(part.failure.as_deref(), Some("permission denied"));
        assert_eq!(part.output, None);
    }

    #[test]
    fn keeps_what_a_finished_command_printed_and_codexs_exit_code_with_it() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-3",
            "title": "pnpm vitest run",
            "kind": "execute",
            "status": "in_progress",
            "rawInput": { "command": "pnpm vitest run", "description": "Run the tests" },
        }))
        .update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tool-3",
            "status": "failed",
            "rawOutput": { "formatted_output": " Tests  2 failed | 40 passed\n", "exit_code": 1 },
        }));
        let part = first_tool(&mut chat);
        assert_eq!(
            part.output,
            Some(OutputSummary {
                lines: 1,
                cut: false,
                exit_code: Some(1),
                image: false
            })
        );
        assert_eq!(part.failure, None);
        assert_eq!(part.tool.description.as_deref(), Some("Run the tests"));
        assert_eq!(
            chat.fold
                .tool_detail("tool-3")
                .and_then(|detail| detail.output),
            Some(ToolOutput {
                text: " Tests  2 failed | 40 passed".into(),
                cut: false,
                exit_code: Some(1),
                image: None,
            })
        );
    }

    #[test]
    fn keeps_the_page_a_finished_page_show_showed_in_place_of_what_it_printed() {
        let page =
            json!({ "id": "0123456789abcdef0123456789abcdef", "title": "Revenue", "height": 300 });
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "page-1", "title": "mcp__sikemux-tools__page_show", "status": "in_progress" }))
            .update(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "page-1",
                "status": "completed",
                "rawOutput": json!({ "page": page, "note": "shown" }).to_string(),
            }));
        let part = first_tool(&mut chat);
        assert_eq!(
            part.page,
            Some(PageRef {
                id: "0123456789abcdef0123456789abcdef".into(),
                title: "Revenue".into(),
                height: Some(300.0),
            })
        );
        assert_eq!(part.output, None);
    }

    #[test]
    fn keeps_a_picture_too_big_to_hold_by_name_rather_than_by_its_bytes() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "agent_message_chunk",
            "messageId": "remote-1",
            "content": { "type": "image", "mimeType": "image/png", "data": "A".repeat(3 * 1024 * 1024), "uri": "file:///tmp/shot.png" },
        }));
        match &chat.rows()[0].parts[0] {
            ChatPart::Content {
                content: ContentBlock::Image { data, uri, .. },
                ..
            } => {
                assert!(data.is_none());
                assert_eq!(uri.as_deref(), Some("file:///tmp/shot.png"));
            }
            other => panic!("expected a content part, got {other:?}"),
        }
    }

    #[test]
    fn streams_a_subagent_session_into_its_own_thread() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "subagent-1", "name": "Explore", "task": "Find the ACP adapter" }))
            .update_in("subagent-1", chunk("Looking", "child-1"))
            .update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "subagent-1", "state": "completed" }));
        let view = chat.view();
        assert_eq!(
            card_of(&view.rows[0].parts[0]).state,
            SubagentState::Completed
        );
        let own = &view.subagents["subagent-1"];
        assert_eq!(
            plain(&own[0].parts),
            vec![text(&format!("{}.0", own[0].id), "Looking")]
        );
    }

    #[test]
    fn keeps_subagent_output_out_of_the_parent_transcript() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "subagent-1", "name": "Explore", "task": "" }))
            .update_in("subagent-1", say("agent_message_chunk", "nested"));
        let rows = chat.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].parts.len(), 1);
    }

    #[test]
    fn ignores_a_tool_update_for_a_call_it_never_saw_open() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-9", "status": "in_progress" }));
        assert!(chat.rows().is_empty());
    }

    #[test]
    fn keeps_a_subagents_own_subagents_inside_its_card() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "parent", "name": "Map pipeline", "task": "Map it" }))
            .update_in("parent", json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "child", "name": "Explore", "task": "Read" }))
            .update_in("child", json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Grep", "status": "in_progress" }))
            .update_in("parent", json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "child", "state": "completed" }));
        let view = chat.view();
        assert_eq!(view.rows.len(), 1);
        assert_eq!(card_of(&view.rows[0].parts[0]).session_id, "parent");
        let inner = card_of(&view.subagents["parent"][0].parts[0]);
        assert_eq!(inner.session_id, "child");
        assert_eq!(inner.state, SubagentState::Completed);
        assert!(matches!(
            view.subagents["child"][0].parts[0],
            ChatPart::Tool { .. }
        ));
    }

    #[test]
    fn stops_the_calls_a_finished_turn_left_mid_flight_but_not_its_subagents() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "subagent-1", "name": "Explore", "task": "Look around" }))
            .update_in("subagent-1", json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-2", "title": "Grep", "status": "in_progress" }))
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Task", "status": "pending" }))
            .turn_completed(Some("cancelled"));
        let view = chat.view();
        let parts = &view.rows[0].parts;
        assert_eq!(card_of(&parts[0]).state, SubagentState::Running);
        let call = tool_of(&parts[1]);
        assert_eq!(call.tool.status, Some(ToolStatus::Cancelled));
        assert!(call.ended_at.is_some());
        let nested = tool_of(&view.subagents["subagent-1"][0].parts[0]);
        assert_eq!(nested.tool.status, Some(ToolStatus::InProgress));
    }

    #[test]
    fn leaves_a_call_that_already_answered_alone_when_the_turn_ends() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Read file", "status": "completed" }));
        assert!(chat.rows_untouched_by(|chat| {
            chat.turn_completed(None);
        }));
    }

    #[test]
    fn tracks_a_background_task_until_it_reaches_an_end_state() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "async_task_spawned",
            "asyncTaskId": "task-1",
            "name": "pnpm test",
            "taskType": "shell",
            "description": "Run the suite",
            "canStop": true,
        }));
        assert_eq!(
            chat.head().tasks,
            vec![AsyncTask {
                async_task_id: "task-1".into(),
                name: "pnpm test".into(),
                task_type: "shell".into(),
                description: "Run the suite".into(),
                state: TaskState::Running,
                can_stop: true,
                summary: None,
                last_tool_name: None,
                output_file_path: None,
                usage: None,
                owner: None,
                agents: Vec::new(),
            }]
        );
        chat.update(json!({ "sessionUpdate": "async_task_progress", "asyncTaskId": "task-1", "summary": "12 files passed" }));
        assert_eq!(
            chat.head().tasks[0].summary.as_deref(),
            Some("12 files passed")
        );
        chat.update(json!({ "sessionUpdate": "async_task_state_update", "asyncTaskId": "task-1", "state": "completed" }));
        assert!(chat.head().tasks.is_empty());
        let rows = chat.rows();
        match rows.last().and_then(|row| row.parts.last()) {
            Some(ChatPart::Notice { notice, .. }) => assert_eq!(
                *notice,
                TaskNotice {
                    name: "pnpm test".into(),
                    state: NoticeState::Completed,
                    summary: Some("12 files passed".into()),
                    task_type: Some("shell".into()),
                    subagent: None,
                }
            ),
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[test]
    fn turns_a_background_agents_notification_into_a_notice_never_its_report() {
        let mut chat = Chat::new();
        chat.update(say(
            "user_message_chunk",
            &[
                "<task-notification>\n<task-id>a41</task-id>\n<status>completed</status>",
                "<result>Research is done.\n\n## Blocking bugs\n\n- one\n- two</result>\n</task-notification>",
                "<system-reminder>\nNot user input.\n\nIgnore.\n</system-reminder>",
            ]
            .join("\n"),
        ));
        let rows = chat.rows();
        let notices: Vec<&TaskNotice> = rows
            .iter()
            .flat_map(|row| &row.parts)
            .map(|part| match part {
                ChatPart::Notice { notice, .. } => notice,
                other => panic!("expected only a notice, got {other:?}"),
            })
            .collect();
        assert_eq!(
            notices,
            vec![&TaskNotice {
                name: "Background task".into(),
                state: NoticeState::Completed,
                summary: None,
                task_type: None,
                subagent: None,
            }]
        );
        assert!(rows.iter().all(|row| row.role != ChatRole::User));
    }

    #[test]
    fn keeps_pasted_markup_with_paragraphs_that_the_harness_did_not_write() {
        let mut chat = Chat::new();
        chat.update(say(
            "user_message_chunk",
            "<div>\n\nwhy is this blank\n</div>",
        ));
        assert_eq!(chat.rows().len(), 1);
    }

    #[test]
    fn keeps_claudes_interrupt_marker_out_of_the_transcript() {
        let mut chat = Chat::new();
        for marker in [
            "[Request interrupted by user]",
            "[Request interrupted by user for tool use]",
        ] {
            chat.update(say("user_message_chunk", marker));
        }
        assert!(chat.rows().is_empty());
    }

    #[test]
    fn keeps_a_message_that_only_mentions_a_tag() {
        let mut chat = Chat::new();
        chat.update(say(
            "user_message_chunk",
            "why does <b>bold</b> render oddly",
        ));
        let rows = chat.rows();
        assert_eq!(
            plain(&rows.last().expect("a row").parts),
            vec![text("m0.0", "why does <b>bold</b> render oddly")]
        );
    }

    #[test]
    fn drops_background_tasks_when_the_session_stops() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1", "name": "pnpm test", "canStop": true }))
            .status("stopped");
        assert!(chat.head().tasks.is_empty());
    }

    #[test]
    fn does_not_mistake_history_replayed_after_the_session_is_ready_for_a_turn() {
        let mut chat = Chat::new();
        chat.ready()
            .update(say("user_message_chunk", "Run the pre-push checks"))
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "pnpm check", "status": "completed" }))
            .update(say("agent_message_chunk", "All pushed."));
        assert!(!chat.head().running);
    }

    #[test]
    fn leaves_ending_a_turn_to_the_turn_events_not_the_usage_report() {
        let mut chat = Chat::new();
        chat.ready().turn_started().update(json!({
            "sessionUpdate": "usage_update",
            "used": 1,
            "size": 10,
            "_meta": { "_claude/origin": { "kind": "task-notification" } },
        }));
        assert!(chat.head().running);
        assert_eq!(
            chat.head().usage,
            Some(ContextUsage {
                used: 1.0,
                size: 10.0,
                cost: None
            })
        );
    }

    #[test]
    fn replaces_slash_commands_when_the_agent_sends_a_new_list() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                { "name": "compact", "description": "Compact context", "input": { "hint": "focus" } },
                { "name": "help", "description": "Show help" },
                { "broken": true },
            ],
        }));
        assert_eq!(
            chat.head().commands,
            vec![
                ChatCommandInfo {
                    name: "compact".into(),
                    description: "Compact context".into(),
                    hint: Some("focus".into()),
                },
                ChatCommandInfo {
                    name: "help".into(),
                    description: "Show help".into(),
                    hint: None,
                },
            ]
        );
    }

    fn ask(request: &str, title: &str) -> Value {
        json!({
            "requestId": request,
            "sessionId": ROOT,
            "toolCall": { "toolCallId": "t1", "title": title },
            "options": [{ "optionId": "allow", "name": "Allow", "kind": "allow_once" }],
        })
    }

    #[test]
    fn keeps_independent_permission_requests_until_each_reply_completes() {
        let mut chat = Chat::new();
        chat.event(
            ChatEventKind::PermissionRequest,
            ask("permission-1", "Run tests"),
        );
        assert_eq!(
            chat.head().permissions,
            vec![PermissionAsk {
                request_id: "permission-1".into(),
                session_id: ROOT.into(),
                tool_call: ToolCall {
                    tool_call_id: "t1".into(),
                    title: "Run tests".into(),
                    ..ToolCall::default()
                }
                .with_row(),
                diff: None,
                options: vec![PermissionOption {
                    option_id: "allow".into(),
                    name: "Allow".into(),
                    kind: PermissionKind::AllowOnce,
                }],
            }]
        );
        chat.fold.forget_permission("permission-1");
        assert!(chat.head().permissions.is_empty());
    }

    #[test]
    fn splits_a_message_into_text_thought_and_content_parts_as_the_kind_changes() {
        let image = json!({ "type": "image", "mimeType": "image/png", "data": "AAAA" });
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "agent_thought_chunk", "messageId": "m1", "content": { "type": "text", "text": "Thinking" } }))
            .update(json!({ "sessionUpdate": "agent_thought_chunk", "messageId": "m1", "content": { "type": "text", "text": " harder" } }))
            .update(chunk("Answer", "m1"))
            .update(json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": image }))
            .update(chunk("After", "m1"));
        let rows = chat.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            plain(&rows[0].parts),
            vec![
                ChatPart::Thought {
                    id: "m0.0".into(),
                    text: "Thinking harder".into(),
                    md: None,
                    cut_bytes: None,
                },
                text("m0.1", "Answer"),
                ChatPart::Content {
                    id: "m0.2".into(),
                    content: ContentBlock::read(&image)
                },
                text("m0.3", "After"),
            ]
        );
    }

    #[test]
    fn opens_a_message_with_a_picture_and_keeps_a_small_ones_bytes() {
        let image = json!({ "type": "image", "mimeType": "image/png", "data": "AAAA" });
        let mut chat = Chat::new();
        chat.update(
            json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": image }),
        );
        assert_eq!(
            chat.rows()[0].parts,
            vec![ChatPart::Content {
                id: "m0.0".into(),
                content: ContentBlock::read(&image)
            }]
        );
    }

    #[test]
    fn drops_a_chunk_that_carries_no_content_block() {
        let mut chat = Chat::new();
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1" }));
        }));
        assert!(chat.untouched_by(|chat| {
            chat.update(
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "text": "no type" } }),
            );
        }));
        assert!(chat.rows().is_empty());
    }

    #[test]
    fn starts_a_new_assistant_message_after_the_users_own_prompt_rather_than_extending_it() {
        let mut chat = Chat::new();
        chat.prompt(json!({ "text": "Hi", "paths": [] }))
            .update(say("agent_message_chunk", "Hello"))
            .update(say("agent_message_chunk", " there"));
        let rows = chat.rows();
        assert_eq!(
            rows.iter().map(|row| row.role).collect::<Vec<_>>(),
            vec![ChatRole::User, ChatRole::Assistant]
        );
        assert_eq!(plain(&rows[1].parts), vec![text("m1.0", "Hello there")]);
    }

    #[test]
    fn times_only_the_answer_to_a_turn_it_watched_run() {
        let mut watched = Chat::running();
        watched.at(1_000).update(chunk("Hel", "m1"));
        watched.at(1_500).update(chunk("lo", "m1"));
        let stream = watched.fold.main.rows[0].stream.expect("timed");
        assert_eq!(
            (stream.started, stream.ended, stream.chars),
            (1_000, 1_500, 5)
        );

        let mut replayed = Chat::new();
        replayed.update(chunk("Hello", "m1"));
        assert!(replayed.fold.main.rows[0].stream.is_none());
    }

    #[test]
    fn drops_a_user_chunk_that_is_blank_or_a_lone_harness_tag() {
        let mut chat = Chat::new();
        chat.update(say("user_message_chunk", "   \n"))
            .update(say(
                "user_message_chunk",
                "<ide_opened_file>src/a.ts</ide_opened_file>",
            ))
            .update(say(
                "user_message_chunk",
                "<command-name>/clear</command-name>\n<command-args></command-args>",
            ));
        assert!(chat.rows().is_empty());
    }

    #[test]
    fn keeps_a_user_message_whose_harness_tag_is_never_closed() {
        let mut chat = Chat::new();
        chat.update(say("user_message_chunk", "<system-reminder> left open"));
        assert_eq!(chat.rows().len(), 1);
    }

    #[test]
    fn replaces_a_call_the_agent_announces_again_instead_of_merging_into_it() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-1",
            "title": "Read a.ts",
            "kind": "read",
            "status": "pending",
            "locations": [{ "path": "/a.ts" }],
        }))
        .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Read b.ts", "status": "in_progress" }));
        let rows = chat.rows();
        assert_eq!(rows[0].parts.len(), 1);
        assert_eq!(
            tool_of(&rows[0].parts[0]).tool,
            ToolCall {
                tool_call_id: "tool-1".into(),
                title: "Read b.ts".into(),
                status: Some(ToolStatus::InProgress),
                ..ToolCall::default()
            }
            .with_row()
        );
    }

    #[test]
    fn keeps_a_calls_title_when_an_update_sends_an_empty_one_and_stays_open_while_it_runs() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Run tests", "status": "pending" }))
            .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "title": "", "status": "in_progress" }));
        let part = first_tool(&mut chat);
        assert_eq!(part.tool.title, "Run tests");
        assert_eq!(part.tool.status, Some(ToolStatus::InProgress));
        assert_eq!(part.ended_at, None);
        assert!(part.started_at.is_some());
    }

    #[test]
    fn keeps_a_finished_calls_end_time_and_drops_payloads_a_late_update_sends() {
        let mut chat = Chat::new();
        chat.at(2_000)
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Grep", "status": "in_progress" }))
            .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed" }));
        chat.at(9_000).update(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tool-1",
            "status": "failed",
            "rawOutput": { "stderr": "late" },
        }));
        let part = first_tool(&mut chat);
        assert_eq!(part.ended_at, Some(2_000));
        assert_eq!(part.tool.status, Some(ToolStatus::Failed));
        assert_eq!(part.failure, None);
    }

    #[test]
    fn settles_a_call_first_seen_already_finished_without_timing_it() {
        let mut chat = Chat::new();
        chat.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tool-1",
            "title": "Grep",
            "status": "failed",
            "rawOutput": { "stderr": "boom" },
        }));
        let part = first_tool(&mut chat);
        assert_eq!(part.started_at, None);
        assert_eq!(part.failure.as_deref(), Some("boom"));
    }

    #[test]
    fn ignores_a_call_with_no_title_or_no_id() {
        let mut chat = Chat::new();
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1" }));
        }));
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "tool_call_update", "status": "completed" }));
        }));
    }

    #[test]
    fn cancels_a_call_that_never_reported_a_status_when_the_turn_ends() {
        let mut chat = Chat::new();
        chat.update(
            json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Grep" }),
        )
        .turn_completed(None);
        assert_eq!(
            first_tool(&mut chat).tool.status,
            Some(ToolStatus::Cancelled)
        );
        assert_eq!(chat.head().stop_reason, None);
    }

    #[test]
    fn leaves_a_finished_subagent_and_plain_text_alone_when_the_turn_ends() {
        let mut chat = Chat::new();
        chat.update(say("agent_message_chunk", "Sending an agent"))
            .update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }))
            .update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-1", "state": "completed" }));
        assert!(chat.rows_untouched_by(|chat| {
            chat.turn_completed(Some("end_turn"));
        }));
        assert_eq!(chat.head().stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn cancels_a_finished_subagents_call_that_was_still_running() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }))
            .update_in("sub-1", json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Grep", "status": "in_progress" }))
            .update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-1", "state": "completed" }))
            .turn_completed(None);
        let view = chat.view();
        assert_eq!(
            card_of(&view.rows[0].parts[0]).state,
            SubagentState::Completed
        );
        let inner = tool_of(&view.subagents["sub-1"][0].parts[0]);
        assert_eq!(inner.tool.status, Some(ToolStatus::Cancelled));
    }

    #[test]
    fn names_an_unnamed_subagent_and_spawns_each_session_once() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }));
        let card = card_of(&chat.rows()[0].parts[0]);
        assert_eq!(
            (card.name.as_str(), card.task.as_str(), card.state),
            ("Subagent", "", SubagentState::Running)
        );
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1", "name": "Other" }));
        }));
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "subagent_spawned" }));
        }));
    }

    #[test]
    fn ignores_a_subagent_state_it_does_not_know_or_for_a_session_it_never_saw() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }));
        for update in [
            json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-1", "state": "exploded" }),
            json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-9", "state": "completed" }),
            json!({ "sessionUpdate": "subagent_state_update", "state": "completed" }),
        ] {
            assert!(chat.untouched_by(|chat| {
                chat.update(update);
            }));
        }
    }

    #[test]
    fn gives_an_unnamed_background_task_defaults_and_starts_each_task_once() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1" }));
        assert_eq!(
            chat.head().tasks,
            vec![AsyncTask {
                async_task_id: "task-1".into(),
                name: "Background task".into(),
                task_type: String::new(),
                description: String::new(),
                state: TaskState::Running,
                can_stop: false,
                summary: None,
                last_tool_name: None,
                output_file_path: None,
                usage: None,
                owner: None,
                agents: Vec::new(),
            }]
        );
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1" }));
        }));
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "async_task_spawned" }));
        }));
    }

    #[test]
    fn updates_a_paused_tasks_details_and_ignores_progress_for_an_unknown_task() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1", "name": "Explore" }))
            .update(json!({
                "sessionUpdate": "async_task_state_update",
                "asyncTaskId": "task-1",
                "state": "paused",
                "description": "Searching",
                "lastToolName": "Grep",
                "outputFilePath": "/tmp/out.log",
                "usage": { "totalTokens": 10, "toolUses": 2, "durationMs": 300 },
            }));
        let paused = chat.head().tasks[0].clone();
        assert_eq!(paused.state, TaskState::Paused);
        assert_eq!(paused.description, "Searching");
        assert_eq!(paused.last_tool_name.as_deref(), Some("Grep"));
        assert_eq!(paused.output_file_path.as_deref(), Some("/tmp/out.log"));
        assert_eq!(
            paused.usage,
            Some(TaskUsage {
                total_tokens: 10,
                tool_uses: 2,
                duration_ms: 300
            })
        );

        chat.update(json!({ "sessionUpdate": "async_task_progress", "asyncTaskId": "task-1", "state": "bogus" }));
        assert_eq!(chat.head().tasks[0], paused);
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "async_task_progress", "asyncTaskId": "task-9", "summary": "x" }));
        }));
    }

    #[test]
    fn writes_a_notice_without_a_summary_when_a_stopped_task_never_gave_one() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1", "name": "Build" }))
            .update(json!({ "sessionUpdate": "async_task_state_update", "asyncTaskId": "task-1", "state": "stopped" }));
        assert!(chat.head().tasks.is_empty());
        match &chat.rows()[0].parts[0] {
            ChatPart::Notice { notice, .. } => assert_eq!(
                *notice,
                TaskNotice {
                    name: "Build".into(),
                    state: NoticeState::Stopped,
                    summary: None,
                    task_type: None,
                    subagent: None,
                }
            ),
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[test]
    fn shows_the_agents_notices_apart_from_its_own_word_on_a_stopped_task() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "notice", "severity": "warning", "title": "Fast mode turned off", "description": "Usage limit." }));
        match &chat.rows()[0].parts[0] {
            ChatPart::AgentNotice { notice, .. } => assert_eq!(
                *notice,
                AgentNotice {
                    severity: NoticeSeverity::Warning,
                    title: "Fast mode turned off".into(),
                    description: Some("Usage limit.".into()),
                }
            ),
            other => panic!("expected an agent notice, got {other:?}"),
        }
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "notice", "severity": "info", "title": "Task stopped by user", "description": "Build." }));
        }));
    }

    #[test]
    fn keeps_the_plan_title_and_config_the_session_reports() {
        let plan = json!({ "sessionUpdate": "plan", "entries": [{ "content": "Write tests", "status": "pending" }] });
        let mut chat = Chat::new();
        chat.update(plan.clone())
            .update(json!({ "sessionUpdate": "session_info_update", "title": "Coverage work" }))
            .update(json!({ "sessionUpdate": "config_option_update", "configOptions": [{ "id": "model" }] }));
        assert_eq!(
            chat.head().plan,
            Some(vec![PlanEntry {
                content: "Write tests".into(),
                priority: PlanPriority::Medium,
                status: PlanStatus::Pending,
            }])
        );
        assert_eq!(chat.head().title.as_deref(), Some("Coverage work"));
        assert_eq!(
            chat.head().setup,
            json!({ "configOptions": [{ "id": "model" }] })
        );
        chat.update(json!({ "sessionUpdate": "session_info_update", "title": null }));
        assert_eq!(chat.head().title.as_deref(), Some("Coverage work"));
    }

    #[test]
    fn clears_slash_commands_when_the_list_is_not_an_array_and_ignores_unknown_updates() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "available_commands_update", "availableCommands": [{ "name": "help", "description": "Help" }] }));
        assert!(chat.untouched_by(|chat| {
            chat.update(json!({ "sessionUpdate": "something_new" }));
        }));
        chat.update(json!({ "sessionUpdate": "available_commands_update" }));
        assert!(chat.head().commands.is_empty());
    }

    #[test]
    fn keeps_a_usage_cost_only_when_both_its_amount_and_currency_make_sense() {
        let usage = |update: Value| {
            let mut chat = Chat::new();
            chat.update(update);
            chat.head().usage.clone()
        };
        let bare = Some(ContextUsage {
            used: 1.0,
            size: 10.0,
            cost: None,
        });
        assert_eq!(
            usage(
                json!({ "sessionUpdate": "usage_update", "used": 1, "size": 10, "cost": { "amount": 2 } })
            ),
            bare
        );
        assert_eq!(
            usage(
                json!({ "sessionUpdate": "usage_update", "used": 1, "size": 10, "cost": { "amount": f64::INFINITY, "currency": "USD" } })
            ),
            bare
        );
        assert_eq!(
            usage(json!({ "sessionUpdate": "usage_update", "used": f64::NAN, "size": 10 })),
            None
        );
    }

    fn busy() -> Chat {
        let mut chat = Chat::running();
        chat.event(
            ChatEventKind::PermissionRequest,
            json!({ "requestId": "p1", "sessionId": ROOT, "toolCall": { "toolCallId": "t1", "title": "Run" }, "options": [] }),
        )
        .update(json!({ "sessionUpdate": "async_task_spawned", "asyncTaskId": "task-1" }))
        .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run", "status": "in_progress" }));
        chat
    }

    #[test]
    fn keeps_a_running_turn_through_a_starting_status_but_ends_it_on_an_error() {
        let mut starting = busy();
        starting.fold.head.error = Some("old".into());
        starting.status("starting");
        assert_eq!(starting.head().connection, ChatConnection::Starting);
        assert!(starting.head().running);
        assert_eq!(starting.head().error, None);
        assert_eq!(starting.head().permissions.len(), 1);
        assert_eq!(starting.head().tasks.len(), 1);
        assert_eq!(
            first_tool(&mut starting).tool.status,
            Some(ToolStatus::InProgress)
        );

        let mut failed = busy();
        failed.fold.head.error = Some("crashed".into());
        failed.status("error");
        assert_eq!(failed.head().connection, ChatConnection::Error);
        assert!(!failed.head().running);
        assert_eq!(failed.head().error.as_deref(), Some("crashed"));
        assert!(failed.head().permissions.is_empty());
        assert!(failed.head().tasks.is_empty());
        assert_eq!(
            first_tool(&mut failed).tool.status,
            Some(ToolStatus::Cancelled)
        );

        let mut errored = busy();
        errored.error(json!({ "message": "boom" }));
        assert_eq!(errored.head().connection, ChatConnection::Error);
    }

    #[test]
    fn keeps_a_ready_connection_ready_when_a_prompt_fails() {
        let mut chat = Chat::new();
        chat.ready()
            .turn_started()
            .error(json!({ "message": "rate limited" }));
        assert_eq!(chat.head().connection, ChatConnection::Ready);
        assert!(!chat.head().running);
        assert_eq!(chat.head().error.as_deref(), Some("rate limited"));
    }

    #[test]
    fn sends_a_prompt_of_only_attachments_as_a_message_without_text() {
        let mut sent = Chat::new();
        sent.at(5_000)
            .prompt(json!({ "text": "  ", "paths": ["/tmp/shot.png"] }));
        assert_eq!(
            sent.rows()[0],
            ChatMessage {
                id: "m0".into(),
                role: ChatRole::User,
                prompt_id: None,
                parts: Vec::new(),
                attachments: vec!["/tmp/shot.png".into()],
                context: Vec::new(),
                letters: Vec::new(),
                sent_at: Some(5_000),
                ended_at: None,
                opens_turn: None,
                meta: None,
                fold: None,
                from_event: Some(0),
                reactions: Vec::new(),
            }
        );
        let mut plain = Chat::new();
        plain.prompt(json!({ "text": "Hi", "paths": [] }));
        assert!(plain.rows()[0].attachments.is_empty());
    }

    #[test]
    fn replaces_a_pending_permission_request_that_is_asked_again() {
        let mut chat = Chat::new();
        chat.event(ChatEventKind::PermissionRequest, ask("p1", "Run"))
            .event(ChatEventKind::PermissionRequest, ask("p1", "Run again"));
        let permissions = &chat.head().permissions;
        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].tool_call.title, "Run again");
    }

    #[test]
    fn keeps_a_subagent_going_after_the_turn_that_started_it_ends() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }))
            .update_in("sub-1", json!({ "sessionUpdate": "tool_call", "toolCallId": "tool-1", "title": "Grep", "status": "in_progress" }));
        assert!(chat.rows_untouched_by(|chat| {
            chat.turn_completed(Some("end_turn"));
        }));
        assert_eq!(chat.head().running_subagents.len(), 1);

        chat.update_in("sub-1", json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-1", "status": "completed" }))
            .update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-1", "state": "completed" }));
        assert!(chat.head().running_subagents.is_empty());
    }

    #[test]
    fn leaves_ending_a_subagent_to_the_host_when_its_agent_goes_away_or_starts_again() {
        let ended = || {
            let mut chat = Chat::new();
            chat.update(
                json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }),
            )
            .update_in("sub-1", say("agent_message_chunk", "Looking"))
            .turn_completed(Some("end_turn"));
            chat
        };
        for state in ["stopped", "error", "starting"] {
            let mut chat = ended();
            chat.status(state);
            assert_eq!(chat.head().running_subagents.len(), 1, "after {state}");
        }
        let mut gone = ended();
        gone.update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "sub-1", "state": "disconnected" }));
        assert!(gone.head().running_subagents.is_empty());
    }

    #[test]
    fn keeps_a_subagents_transcript_as_is_when_its_session_sends_an_update_it_cannot_place() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "sub-1" }));
        let spawned = chat.rows()[0].parts.clone();
        chat.update_in("sub-1", json!({ "sessionUpdate": "tool_call_update", "toolCallId": "tool-9", "status": "completed" }));
        let view = chat.view();
        assert_eq!(view.rows[0].parts, spawned);
        assert!(view.subagents["sub-1"].is_empty());
    }

    #[test]
    fn says_in_the_transcript_which_account_the_chat_moved_to_and_why() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "account_switched", "account": "work", "label": "Work", "from": "Personal", "reason": "limit" }));
        let rows = chat.rows();
        assert_eq!(rows.len(), 1);
        match &rows[0].parts[0] {
            ChatPart::Account { moved, .. } => assert_eq!(
                *moved,
                AccountMove {
                    label: "Work".into(),
                    from: Some("Personal".into()),
                    reason: MoveReason::Limit,
                }
            ),
            other => panic!("expected an account move, got {other:?}"),
        }
    }

    #[test]
    fn keeps_why_a_turn_failed_until_the_chat_runs_again() {
        let mut chat = Chat::new();
        chat.ready().error(json!({ "message": "You've hit your limit", "failure": { "kind": "limit", "account": "personal" } }));
        assert_eq!(
            chat.head().failure,
            Some(ChatFailure {
                kind: FailureKind::Limit,
                details: None,
                account: Some("personal".into()),
                resumes_at: None,
            })
        );
        chat.status("starting");
        assert_eq!(chat.head().failure, None);
        assert_eq!(chat.head().error, None);
    }

    mod rewinding_to_a_message_the_person_edits {
        use super::*;

        fn answered(chat: &mut Chat, said: &str) {
            chat.turn_started()
                .update_in("s", chunk(said, &format!("a-{said}")));
        }

        fn first_texts(rows: &[ChatMessage]) -> Vec<Option<String>> {
            rows.iter()
                .map(|row| match row.parts.first() {
                    Some(ChatPart::Text { text, .. }) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn drops_the_edited_message_and_everything_after_it() {
            let mut chat = Chat::new();
            chat.prompt(json!({ "text": "first", "paths": [], "messageId": "m1" }));
            answered(&mut chat, "one");
            chat.prompt(json!({ "text": "second", "paths": [], "messageId": "m2" }));
            answered(&mut chat, "two");
            let rows = chat.rows();
            assert_eq!(
                rows.iter()
                    .map(|row| row.prompt_id.as_deref())
                    .collect::<Vec<_>>(),
                vec![Some("m1"), None, Some("m2"), None]
            );

            chat.fold.rewind("m2");
            let changes = chat.flush();
            assert!(changes.contains(&ChatChange::Drop {
                from: rows[2].id.clone()
            }));
            assert_eq!(
                first_texts(&chat.rows()),
                vec![Some("first".into()), Some("one".into())]
            );
        }

        #[test]
        fn drops_a_replayed_message_another_device_edited() {
            let mut chat = Chat::new();
            chat.update_in(
                "s",
                json!({ "sessionUpdate": "user_message_chunk", "messageId": "u1", "content": { "type": "text", "text": "hello" } }),
            );
            answered(&mut chat, "hi");
            assert_eq!(chat.rows()[0].prompt_id.as_deref(), Some("u1"));
            chat.update_in(
                "s",
                json!({ "sessionUpdate": "message_rewound", "messageId": "u1" }),
            );
            assert!(chat.rows().is_empty());
        }

        #[test]
        fn leaves_the_transcript_alone_for_a_message_it_does_not_have() {
            let mut chat = Chat::new();
            chat.prompt(json!({ "text": "first", "paths": [], "messageId": "m1" }));
            let before = chat.rows();
            assert!(chat.rows_untouched_by(|chat| chat.fold.rewind("gone")));
            assert_eq!(chat.rows(), before);
        }
    }
}

mod acp_events {
    use super::*;

    #[test]
    fn reads_any_other_status_as_still_connecting() {
        for payload in [json!({ "state": "warming" }), json!({})] {
            let mut chat = Chat::new();
            chat.event(ChatEventKind::Status, payload);
            assert_eq!(chat.head().connection, ChatConnection::Connecting);
        }
    }

    fn valid() -> Value {
        json!({
            "requestId": "r1",
            "sessionId": "s1",
            "toolCall": { "toolCallId": "t1", "title": "Run cargo test", "kind": "execute" },
            "options": [
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
            ],
        })
    }

    fn asked(payload: Value) -> Vec<PermissionAsk> {
        let mut chat = Chat::new();
        chat.event(ChatEventKind::PermissionRequest, payload);
        chat.head().permissions.clone()
    }

    fn with(field: &str, value: Value) -> Value {
        let mut payload = valid();
        payload[field] = value;
        payload
    }

    fn option(id: &str, name: &str, kind: &str) -> PermissionOption {
        PermissionOption {
            option_id: id.into(),
            name: name.into(),
            kind: serde_json::from_value(json!(kind)).unwrap(),
        }
    }

    #[test]
    fn reads_a_well_formed_permission_request() {
        assert_eq!(
            asked(valid()),
            vec![PermissionAsk {
                request_id: "r1".into(),
                session_id: "s1".into(),
                tool_call: ToolCall {
                    tool_call_id: "t1".into(),
                    title: "Run cargo test".into(),
                    kind: Some("execute".into()),
                    ..ToolCall::default()
                }
                .with_row(),
                diff: None,
                options: vec![
                    option("allow", "Allow", "allow_once"),
                    option("reject", "Reject", "reject_once")
                ],
            }]
        );
    }

    #[test]
    fn names_an_untitled_tool_call() {
        assert_eq!(
            asked(with("toolCall", json!({ "toolCallId": "t1" })))[0]
                .tool_call
                .title,
            "Agent tool"
        );
    }

    #[test]
    fn drops_options_that_are_missing_a_field() {
        let options = json!([{ "optionId": "allow", "name": "Allow", "kind": "allow_once" }, { "optionId": "half", "name": "Half" }, "junk", null]);
        assert_eq!(
            asked(with("options", options))[0].options,
            vec![option("allow", "Allow", "allow_once")]
        );
    }

    #[test]
    fn refuses_a_request_missing_what_it_needs_to_be_answered() {
        let mut missing = valid();
        missing
            .as_object_mut()
            .expect("an object")
            .remove("requestId");
        assert!(asked(missing).is_empty());
        assert!(asked(with("sessionId", json!(7))).is_empty());
        assert!(asked(with("toolCall", Value::Null)).is_empty());
        assert!(asked(with("toolCall", json!({ "title": "no id" }))).is_empty());
        assert!(asked(with("options", json!("allow"))).is_empty());
    }

    #[test]
    fn shows_a_prompt_from_another_device_the_way_a_typed_one_shows() {
        let mut chat = Chat::new();
        chat.prompt(json!({ "text": "fix the test", "paths": ["/tmp/a.ts", 3] }));
        let rows = chat.rows();
        assert_eq!(rows[0].role, ChatRole::User);
        assert_eq!(plain(&rows[0].parts), vec![text("m0.0", "fix the test")]);
        assert_eq!(rows[0].attachments, vec!["/tmp/a.ts".to_owned()]);
    }

    #[test]
    fn ignores_a_prompt_with_no_text() {
        let mut chat = Chat::new();
        assert!(chat.untouched_by(|chat| {
            chat.prompt(json!({ "paths": [] }));
        }));
        assert!(chat.rows().is_empty());
    }

    #[test]
    fn keeps_why_a_turn_failed_when_the_account_was_to_blame() {
        let mut chat = Chat::new();
        chat.error(json!({ "message": "Not logged in", "failure": { "kind": "signIn", "account": "Work" } }));
        assert_eq!(chat.head().error.as_deref(), Some("Not logged in"));
        assert_eq!(
            chat.head().failure,
            Some(ChatFailure {
                kind: FailureKind::SignIn,
                details: None,
                account: Some("Work".into()),
                resumes_at: None,
            })
        );
    }
}

mod tool_output {
    use super::*;

    fn output(raw_output: Value) -> Option<ToolOutput> {
        tools::tool_output("ls", Some("execute"), Some(&raw_output), None)
    }

    fn printed(text: &str) -> Option<ToolOutput> {
        Some(ToolOutput {
            text: text.into(),
            cut: false,
            exit_code: None,
            image: None,
        })
    }

    #[test]
    fn reads_claudes_result_whether_it_arrives_as_a_string_or_as_blocks() {
        assert_eq!(output(json!("a\nb\n")), printed("a\nb"));
        assert_eq!(
            output(json!([{ "type": "text", "text": "one" }, { "type": "text", "text": "two" }]))
                .map(|output| output.text),
            Some("one\ntwo".into())
        );
    }

    #[test]
    fn reads_codexs_formatted_output_and_an_mcp_calls_result() {
        assert_eq!(
            output(json!({ "formatted_output": "done", "exit_code": 0 })),
            Some(ToolOutput {
                text: "done".into(),
                cut: false,
                exit_code: Some(0),
                image: None,
            })
        );
        let mcp = json!({ "result": { "content": [{ "type": "text", "text": "3 hits" }] }, "error": null });
        assert_eq!(
            tools::tool_output("mcp.docs.search", Some("execute"), Some(&mcp), None)
                .map(|output| output.text),
            Some("3 hits".into())
        );
    }

    #[test]
    fn keeps_the_picture_an_mcp_call_sent_back() {
        let raw = json!([{ "type": "image", "source": { "type": "base64" } }]);
        let content = json!([{ "type": "content", "content": { "type": "image", "data": "SEVMTE8=", "mimeType": "image/png" } }]);
        assert_eq!(
            tools::tool_output(
                "mcp__sikemux-tools__browser_screenshot",
                Some("other"),
                Some(&raw),
                Some(&content)
            ),
            Some(ToolOutput {
                text: String::new(),
                cut: false,
                exit_code: None,
                image: Some(OutputImage {
                    data: "SEVMTE8=".into(),
                    mime_type: "image/png".into(),
                }),
            })
        );
    }

    #[test]
    fn says_an_empty_command_printed_nothing_rather_than_nothing_at_all() {
        assert_eq!(output(json!("")), printed(""));
    }

    #[test]
    fn keeps_the_head_of_a_long_output_and_says_it_was_cut() {
        let long: Vec<String> = (0..1000).map(|index| format!("line {index}")).collect();
        let kept = output(json!(long.join("\n"))).expect("output");
        assert!(kept.cut);
        assert_eq!(kept.text.split('\n').count(), 400);
        assert!(kept.text.starts_with("line 0\n"));
    }

    #[test]
    fn joins_what_a_command_wrote_to_both_streams_skipping_an_empty_one() {
        assert_eq!(
            output(json!({ "stdout": "built", "stderr": "1 warning" })).map(|output| output.text),
            Some("built\n1 warning".into())
        );
        assert_eq!(
            output(json!({ "stdout": "", "stderr": "boom" })).map(|output| output.text),
            Some("boom".into())
        );
    }

    #[test]
    fn reads_an_error_given_as_text_or_as_an_object_with_a_message() {
        assert_eq!(
            output(json!({ "error": "timed out" })).map(|output| output.text),
            Some("timed out".into())
        );
        assert_eq!(
            output(json!({ "error": { "message": "denied" } })).map(|output| output.text),
            Some("denied".into())
        );
    }

    #[test]
    fn has_nothing_to_show_when_the_call_handed_back_nothing_it_can_read() {
        assert_eq!(output(json!({ "exit_code": 0 })), None);
        assert_eq!(output(json!(42)), None);
        assert_eq!(output(json!([{ "type": "image" }])), None);
    }

    #[test]
    fn keeps_the_head_of_one_very_long_line_and_says_it_was_cut() {
        let kept = output(json!("x".repeat(20_000))).expect("output");
        assert!(kept.cut);
        assert_eq!(kept.text.len(), 16_000);
    }

    #[test]
    fn lets_go_of_a_picture_too_big_or_not_a_picture_or_with_no_bytes() {
        let mcp = |content: Value| {
            tools::tool_output("mcp__x__shot", Some("other"), None, Some(&content))
        };
        assert_eq!(
            mcp(
                json!([{ "type": "content", "content": { "type": "image", "data": "A".repeat(1024 * 1024 + 1), "mimeType": "image/png" } }])
            ),
            None
        );
        assert_eq!(
            mcp(
                json!([{ "type": "content", "content": { "type": "image", "data": "AAAA", "mimeType": "text/plain" } }])
            ),
            None
        );
        assert_eq!(
            mcp(
                json!([{ "type": "content", "content": { "type": "image", "mimeType": "image/png" } }, "stray"])
            ),
            None
        );
    }

    #[test]
    fn leaves_reads_and_edits_alone_since_their_rows_already_show_what_they_touched() {
        assert_eq!(
            tools::tool_output("src/app.ts", Some("read"), Some(&json!("export {}")), None),
            None
        );
        assert_eq!(
            tools::tool_output("src/app.ts", Some("edit"), Some(&json!("ok")), None),
            None
        );
    }

    fn description(kind: &str, raw_input: Value) -> Option<String> {
        tools::call(&record(
            json!({ "toolCallId": "tool-1", "title": "ls", "kind": kind, "rawInput": raw_input }),
        ))
        .description
    }

    #[test]
    fn reads_what_claude_said_a_command_is_for() {
        assert_eq!(
            description(
                "execute",
                json!({ "command": "pnpm build", "description": " Build the site " })
            ),
            Some("Build the site".into())
        );
    }

    #[test]
    fn has_no_description_for_a_codex_command_or_a_call_that_is_not_a_command() {
        assert_eq!(
            description(
                "execute",
                json!({ "command": "pnpm build", "cwd": "/repo" })
            ),
            None
        );
        assert_eq!(
            description("read", json!({ "description": "Read it" })),
            None
        );
    }
}

mod tool_duration {
    use super::*;

    #[test]
    fn times_a_call_it_watched_run() {
        let mut chat = Chat::new();
        chat.at(1_000)
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "cargo check", "status": "pending" }));
        chat.at(43_000)
            .update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }));
        let part = first_tool(&mut chat);
        assert_eq!(
            part.ended_at.unwrap_or(0) - part.started_at.unwrap_or(0),
            42_000
        );
    }

    #[test]
    fn claims_no_time_for_a_call_that_was_already_over_when_the_session_reloaded() {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "title": "pnpm rust:test", "status": "completed" }));
        let part = first_tool(&mut chat);
        assert_eq!(part.started_at, None);
        assert_eq!(part.tool.status, Some(ToolStatus::Completed));
    }
}

mod row_meta {
    use super::*;

    fn last_meta(chat: &mut Chat) -> Option<RowMeta> {
        chat.rows().last().and_then(|row| row.meta.clone())
    }

    fn rate(chat: &mut Chat) -> Option<f64> {
        last_meta(chat).expect("the answer's line").rate
    }

    #[test]
    fn reads_the_rate_off_a_turn_it_watched_stream() {
        let mut chat = Chat::running();
        chat.at(0).update(chunk(&"a".repeat(40), "m1"));
        chat.at(2_000)
            .update(chunk(&"b".repeat(40), "m1"))
            .turn_completed(None);
        let rate = rate(&mut chat).expect("a rate");
        assert!((rate - 10.0).abs() < 1e-9, "{rate}");
    }

    #[test]
    fn times_the_writing_and_not_the_tool_call_waited_on_in_the_middle_of_it() {
        let mut chat = Chat::running();
        chat.at(0).update(chunk(&"a".repeat(40), "m1"));
        chat.at(1_000).update(chunk(&"b".repeat(40), "m1"));
        chat.at(60_000)
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "cargo check", "status": "completed" }))
            .update(chunk(&"c".repeat(40), "m2"));
        chat.at(61_000)
            .update(chunk(&"d".repeat(40), "m2"))
            .turn_completed(None);
        let rate = rate(&mut chat).expect("a rate");
        assert!((rate - 20.0).abs() < 1e-9, "{rate}");
    }

    #[test]
    fn times_nothing_for_history_replayed_after_a_reconnect() {
        let mut chat = Chat::new();
        chat.update(chunk(&"a".repeat(400), "m1"));
        assert!(chat.fold.main.rows[0].stream.is_none());
        assert_eq!(rate(&mut chat), None);
    }

    #[test]
    fn gives_no_rate_for_a_burst_too_short_to_measure() {
        let mut chat = Chat::running();
        chat.at(0).update(chunk("hello", "m1"));
        chat.at(100)
            .update(chunk(" there", "m1"))
            .turn_completed(None);
        assert_eq!(rate(&mut chat), None);
    }

    #[test]
    fn puts_a_whole_answers_line_where_it_ends_and_none_in_the_middle_of_one() {
        let mut chat = Chat::new();
        chat.prompt(json!({ "text": "wire it up", "paths": [] }))
            .update(chunk("Looking now.", "m1"))
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "rg wire", "status": "completed" }))
            .update(chunk("Wired.", "m2"))
            .turn_completed(None);
        let rows = chat.rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].meta, None);
        assert_eq!(
            rows[2].meta.as_ref().map(|meta| meta.from.as_str()),
            Some(rows[1].id.as_str())
        );
    }

    #[test]
    fn stamps_a_prompt_with_when_it_was_sent_and_its_answer_with_when_the_turn_finished_and_how_long_it_took(
    ) {
        let sent = 1_790_000_000_000;
        let mut chat = Chat::new();
        chat.at(sent)
            .prompt(json!({ "text": "check the build", "paths": [] }))
            .turn_started();
        chat.at(sent + 4_000).update(chunk("Looking.", "m1"));
        chat.at(sent + 120_000)
            .update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "cargo check", "status": "completed" }))
            .update(chunk("It builds.", "m2"));
        assert_eq!(last_meta(&mut chat), None);

        let finished = sent + (4 * 60 + 52) * 1000;
        chat.at(finished).turn_completed(None);
        let rows = chat.rows();
        assert_eq!(rows[0].sent_at, Some(sent));
        let meta = rows
            .last()
            .and_then(|row| row.meta.clone())
            .expect("the answer's line");
        assert_eq!(meta.at, Some(finished));
        assert_eq!(meta.took, Some((4 * 60 + 52) * 1000));
    }

    #[test]
    fn has_no_time_for_history_replayed_after_a_reconnect() {
        let mut chat = Chat::new();
        chat.update(chunk("from before", "m1")).turn_completed(None);
        let rows = chat.rows();
        assert_eq!(rows[0].sent_at, None);
        assert_eq!(rows[0].ended_at, None);
        let meta = rows[0].meta.clone().expect("the answer's line");
        assert_eq!((meta.at, meta.took), (None, None));
    }
}

mod diff {
    use super::*;

    fn signs(lines: &[DiffLine]) -> Vec<String> {
        lines
            .iter()
            .map(|line| format!("{}{}", line.sign, line.text))
            .collect()
    }

    fn marked(line: &DiffLine) -> &str {
        let mark = line.mark.as_ref().expect("a marked span");
        &line.text[mark[0] as usize..mark[1] as usize]
    }

    #[test]
    fn numbers_both_sides_and_keeps_what_did_not_change() {
        let lines = tools::diff_lines("a\nb\nc\n", "a\nB\nc\n").expect("lines");
        assert_eq!(signs(&lines), vec![" a", "-b", "+B", " c"]);
        assert_eq!((lines[0].old_line, lines[0].new_line), (Some(1), Some(1)));
        assert_eq!(lines[1].old_line, Some(2));
        assert_eq!(lines[2].new_line, Some(2));
        assert_eq!((lines[3].old_line, lines[3].new_line), (Some(3), Some(3)));
    }

    #[test]
    fn marks_the_span_that_changed_when_one_line_replaces_one_line() {
        let lines = tools::diff_lines(
            "    background: var(--pane);\n",
            "    background: transparent;\n",
        )
        .expect("lines");
        let deleted = lines
            .iter()
            .find(|line| line.sign == "-")
            .expect("a deleted line");
        let added = lines
            .iter()
            .find(|line| line.sign == "+")
            .expect("an added line");
        assert_eq!(marked(deleted), "var(--pane)");
        assert_eq!(marked(added), "transparent");
    }

    #[test]
    fn leaves_two_unrelated_lines_unmarked_rather_than_inventing_a_span() {
        let lines = tools::diff_lines("alpha\n", "beta\n").expect("lines");
        assert!(lines.iter().all(|line| line.mark.is_none()));
    }

    #[test]
    fn reads_an_insertion_as_added_lines_not_as_a_rewrite() {
        let lines = tools::diff_lines("one\ntwo\n", "one\nextra\ntwo\n").expect("lines");
        assert_eq!(signs(&lines), vec![" one", "+extra", " two"]);
    }

    #[test]
    fn gives_up_on_a_change_too_large_to_read_in_a_transcript() {
        let big: Vec<String> = (0..900).map(|index| format!("line {index}")).collect();
        let big = big.join("\n");
        assert_eq!(tools::diff_lines(&big, &format!("{big}\nmore")), None);
    }

    #[test]
    fn keeps_a_line_both_sides_share_in_the_middle_of_a_rewrite() {
        let lines = tools::diff_lines("head\na\nkeep\nb\ntail\n", "head\nx\nkeep\ny\nz\ntail\n")
            .expect("lines");
        assert_eq!(
            signs(&lines),
            vec![" head", "-a", "+x", " keep", "-b", "+y", "+z", " tail"]
        );
        let keep = lines
            .iter()
            .find(|line| line.text == "keep")
            .expect("the kept line");
        assert_eq!((keep.old_line, keep.new_line), (Some(3), Some(3)));
    }

    #[test]
    fn reads_a_line_moved_up_past_another_as_one_added_and_one_deleted() {
        let lines = tools::diff_lines("a\nb\nc\n", "c\na\nb\n").expect("lines");
        assert_eq!(signs(&lines), vec!["+c", " a", " b", "-c"]);
    }

    #[test]
    fn marks_nothing_inside_a_block_of_several_lines_replaced_by_several() {
        let lines = tools::diff_lines(
            "const first = 1;\nconst second = 2;\n",
            "const first = 10;\nconst second = 20;\n",
        )
        .expect("lines");
        assert_eq!(
            lines
                .iter()
                .map(|line| line.sign.as_str())
                .collect::<Vec<_>>(),
            vec!["-", "-", "+", "+"]
        );
        assert!(lines.iter().all(|line| line.mark.is_none()));
    }

    #[test]
    fn does_not_mark_a_very_long_line() {
        let long = "x".repeat(250);
        let lines = tools::diff_lines(&format!("{long}a\n"), &format!("{long}b\n")).expect("lines");
        assert!(lines.iter().all(|line| line.mark.is_none()));
    }

    #[test]
    fn reads_an_empty_file_emptied_as_no_lines_at_all() {
        assert_eq!(tools::diff_lines("", ""), Some(Vec::new()));
        let lines = tools::diff_lines("", "only\n").expect("lines");
        assert_eq!(
            lines
                .iter()
                .map(|line| line.sign.as_str())
                .collect::<Vec<_>>(),
            vec!["+"]
        );
    }

    fn counted(diff: Option<sikemux_wire::chat::ToolDiff>) -> Option<(String, u64, u64)> {
        diff.map(|diff| (diff.path, diff.adds, diff.dels))
    }

    fn of_content(content: Value) -> Option<(String, u64, u64)> {
        counted(tools::tool_diff(Some(&content), None))
    }

    fn of_input(input: Value) -> Option<(String, u64, u64)> {
        counted(tools::tool_diff(None, Some(&input)))
    }

    fn change(path: &str, adds: u64, dels: u64) -> Option<(String, u64, u64)> {
        Some((path.into(), adds, dels))
    }

    #[test]
    fn takes_the_change_the_adapter_already_computed() {
        assert_eq!(
            of_content(json!([{
                "type": "diff",
                "path": "src/styles/stage.css",
                "oldText": "background: var(--pane);\n",
                "newText": "background: transparent;\n",
            }])),
            change("src/styles/stage.css", 1, 1)
        );
    }

    #[test]
    fn falls_back_to_what_the_tool_was_asked_to_do() {
        assert_eq!(
            of_input(
                json!({ "file_path": "src/app.ts", "old_string": "a\n", "new_string": "a\nb\n" })
            ),
            change("src/app.ts", 1, 0)
        );
    }

    #[test]
    fn reads_a_write_of_a_new_file_as_all_additions() {
        assert_eq!(
            of_input(json!({ "file_path": "src/new.ts", "content": "one\ntwo\n" })),
            change("src/new.ts", 2, 0)
        );
    }

    #[test]
    fn finds_a_diff_wrapped_in_a_content_block_and_skips_the_blocks_that_are_not_one() {
        assert_eq!(
            of_content(json!([
                "stray",
                { "type": "content", "content": { "type": "text", "text": "Editing" } },
                { "type": "diff", "newText": "no path\n" },
                { "type": "content", "content": { "type": "diff", "path": "src/new.ts", "newText": "one\n" } },
            ])),
            change("src/new.ts", 1, 0)
        );
    }

    #[test]
    fn falls_back_to_the_raw_input_when_the_content_has_no_diff() {
        let content = json!([{ "type": "text", "text": "ok" }]);
        let input = json!({ "path": "a.ts", "oldText": "a\n", "newText": "b\n" });
        assert_eq!(
            counted(tools::tool_diff(Some(&content), Some(&input))),
            change("a.ts", 1, 1)
        );
        assert_eq!(
            of_input(json!({ "filePath": "b.ts", "content": "x\n" })),
            change("b.ts", 1, 0)
        );
    }

    #[test]
    fn has_nothing_to_show_when_it_cannot_tell_what_was_written_or_it_is_too_big_to_read() {
        assert_eq!(
            of_input(json!({ "file_path": "a.ts", "old_string": "a\n" })),
            None
        );
        assert_eq!(of_input(json!("edit a.ts")), None);
        let big: Vec<String> = (0..1300).map(|index| format!("line {index}")).collect();
        assert_eq!(
            of_input(json!({ "file_path": "big.ts", "content": big.join("\n") })),
            None
        );
    }

    #[test]
    fn has_nothing_to_show_for_a_call_that_changed_no_file() {
        assert_eq!(of_input(json!({ "command": "pnpm test" })), None);
        assert_eq!(
            of_input(
                json!({ "file_path": "src/app.ts", "old_string": "same\n", "new_string": "same\n" })
            ),
            None
        );
    }

    #[test]
    fn says_no_failure_for_a_call_that_did_not_fail() {
        assert_eq!(
            tools::tool_failure(Some("completed"), Some(&json!("error"))),
            None
        );
    }

    #[test]
    fn reads_a_failure_from_plain_text_or_from_whichever_field_the_adapter_used() {
        let failed = |raw: Value| tools::tool_failure(Some("failed"), Some(&raw));
        assert_eq!(
            failed(json!(" no such file \n")),
            Some("no such file".into())
        );
        assert_eq!(
            failed(json!({ "output": "out", "stderr": "err" })),
            Some("out".into())
        );
        assert_eq!(failed(json!({ "error": "denied" })), Some("denied".into()));
    }

    #[test]
    fn says_no_failure_when_it_left_no_words_behind() {
        assert_eq!(
            tools::tool_failure(Some("failed"), Some(&json!("   "))),
            None
        );
        assert_eq!(
            tools::tool_failure(Some("failed"), Some(&json!({ "code": 1 }))),
            None
        );
        assert_eq!(tools::tool_failure(Some("failed"), None), None);
    }

    #[test]
    fn cuts_a_long_failure_short() {
        assert_eq!(
            tools::tool_failure(Some("failed"), Some(&json!("e".repeat(500)))),
            Some(format!("{}…", "e".repeat(400)))
        );
    }
}

mod pages {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn shown(
        title: &str,
        status: &str,
        raw_output: Option<Value>,
        content: Option<Value>,
    ) -> Option<PageRef> {
        tools::tool_page(title, Some(status), raw_output.as_ref(), content.as_ref())
    }

    fn printed() -> Option<Value> {
        Some(json!(
            json!({ "page": { "id": ID, "title": "Revenue", "height": 412 }, "note": "shown" })
                .to_string()
        ))
    }

    const PAGE_SHOW: &str = "mcp__sikemux-tools__page_show";

    #[test]
    fn reads_the_page_a_finished_page_show_printed() {
        assert_eq!(
            shown(PAGE_SHOW, "completed", printed(), None),
            Some(PageRef {
                id: ID.into(),
                title: "Revenue".into(),
                height: Some(412.0),
            })
        );
    }

    #[test]
    fn reads_it_from_mcp_blocks_and_from_content_when_the_host_keeps_it_there() {
        let text = json!({ "page": { "id": ID, "title": "Revenue" } }).to_string();
        let page = Some(PageRef {
            id: ID.into(),
            title: "Revenue".into(),
            height: None,
        });
        assert_eq!(
            shown(
                PAGE_SHOW,
                "completed",
                Some(json!({ "result": { "content": [{ "type": "text", "text": text }] } })),
                None
            ),
            page
        );
        assert_eq!(
            shown(
                PAGE_SHOW,
                "completed",
                None,
                Some(json!([{ "type": "content", "content": { "type": "text", "text": text } }]))
            ),
            page
        );
    }

    #[test]
    fn finds_a_page_under_any_hosts_way_of_naming_the_tool() {
        assert!(shown("sikemux-tools.page_show", "completed", printed(), None).is_some());
    }

    #[test]
    fn ignores_other_tools_unfinished_calls_and_anything_that_is_not_a_page_id() {
        assert_eq!(
            shown("mcp__sikemux-tools__ui_open", "completed", printed(), None),
            None
        );
        assert_eq!(shown(PAGE_SHOW, "in_progress", printed(), None), None);
        let escape = json!({ "page": { "id": "../../etc/passwd", "title": "x" } }).to_string();
        assert_eq!(
            shown(PAGE_SHOW, "completed", Some(json!(escape)), None),
            None
        );
        assert_eq!(
            shown(
                PAGE_SHOW,
                "completed",
                Some(json!("path must be absolute")),
                None
            ),
            None
        );
    }
}

mod prompt_context {
    use super::*;

    const URI: &str = "https://github.com/o/r/issues/12";

    fn issue() -> SentContext {
        SentContext {
            uri: URI.into(),
            title: "Login crashes".into(),
        }
    }

    #[test]
    fn reads_back_sections_written_into_the_text() {
        let sent = format!("fix this\n\n### Login crashes\n{URI}\n\n```\nIssue #12: Login crashes\n\nIt crashes.\n```");
        assert_eq!(
            prompt::split_sent_context(&sent),
            ("fix this".to_owned(), vec![issue()])
        );
    }

    #[test]
    fn reads_back_what_claudes_adapter_wrote_for_an_embedded_item() {
        let sent = format!("fix this{URI}\n<context ref=\"{URI}\">\nIssue #12: Login crashes\n\nIt crashes.\n</context>");
        assert_eq!(
            prompt::split_sent_context(&sent),
            (
                "fix this".to_owned(),
                vec![SentContext {
                    uri: URI.into(),
                    title: "Issue #12: Login crashes".into(),
                }]
            )
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        assert_eq!(
            prompt::split_sent_context("### Heading\nnot a section"),
            ("### Heading\nnot a section".to_owned(), Vec::new())
        );
    }
}

mod transcript {
    use super::*;

    const TASK: &str = "Find the bug\nThen explain it";

    fn spawned() -> Chat {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": "s1", "name": "explorer", "task": TASK }));
        chat
    }

    #[test]
    fn reports_the_last_tool_the_subagent_called() {
        let mut chat = spawned();
        chat.update_in("s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "a", "title": "/src/one.ts", "kind": "read", "status": "completed" }))
            .update_in("s1", json!({ "sessionUpdate": "tool_call", "toolCallId": "b", "title": "/src/two.ts", "kind": "read", "status": "completed" }))
            .update_in("s1", say("agent_message_chunk", "c"));
        let card = card_of(&chat.rows()[0].parts[0]);
        assert_eq!(card.calls, 2);
        let last = card.last_tool.expect("its last call");
        assert_eq!(
            (last.title.as_str(), last.kind.as_deref()),
            ("/src/two.ts", Some("read"))
        );
    }

    #[test]
    fn falls_back_to_its_task_before_it_has_called_anything() {
        let mut chat = spawned();
        chat.update_in("s1", say("agent_message_chunk", "a"));
        let card = card_of(&chat.rows()[0].parts[0]);
        assert_eq!(card.last_tool, None);
        assert_eq!(card.task, TASK);
    }

    #[test]
    fn lists_only_subagents_still_at_work_in_transcript_order() {
        let spawn = |session: &str| json!({ "sessionUpdate": "subagent_spawned", "subagentSessionId": session });
        let mut chat = Chat::new();
        chat.update(spawn("s1"))
            .update(say("agent_message_chunk", "x"))
            .update(spawn("s2"))
            .update(spawn("s3"))
            .update(json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": "s2", "state": "completed" }));
        let running: Vec<&str> = chat
            .head()
            .running_subagents
            .iter()
            .map(|card| card.session_id.as_str())
            .collect();
        assert_eq!(running, vec!["s1", "s3"]);
    }
}

mod tool_labels {
    use super::*;

    fn kept_while_running_and_once_finished(
        title: &str,
        raw_input: Value,
    ) -> (Option<Value>, Option<Value>) {
        let mut chat = Chat::new();
        chat.update(json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": title, "status": "in_progress", "rawInput": raw_input }));
        let live = first_tool(&mut chat).tool.raw_input;
        chat.update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }));
        (live, first_tool(&mut chat).tool.raw_input)
    }

    #[test]
    fn draws_the_same_row_from_the_arguments_a_finished_call_keeps() {
        let (live, kept) = kept_while_running_and_once_finished(
            "mcp__sikemux-tools__browser_upload",
            json!({ "paths": ["/tmp/a.png"], "index": 3, "report": "full" }),
        );
        assert_eq!(kept, live);
        assert_eq!(
            kept.as_ref().map(|kept| &kept["paths"]),
            Some(&json!(["/tmp/a.png"]))
        );
        assert_eq!(kept.as_ref().map(|kept| &kept["index"]), Some(&json!(3)));

        let (_, steps) = kept_while_running_and_once_finished(
            "mcp__sikemux-tools__browser_act",
            json!({ "steps": [{ "action": "click" }, { "action": "press" }] }),
        );
        assert_eq!(steps, Some(json!({ "steps": 2 })));
    }

    #[test]
    fn keeps_the_arguments_an_agent_puts_beside_the_server_and_tool() {
        let (_, kept) = kept_while_running_and_once_finished(
            "mcp__sikemux-tools__browser_navigate",
            json!({ "server": "sikemux-tools", "tool": "browser_navigate", "arguments": { "url": "https://a.dev" } }),
        );
        assert_eq!(
            kept.map(|kept| kept["arguments"].clone()),
            Some(json!({ "url": "https://a.dev" }))
        );
    }
}

fn row_of(title: &str, kind: Option<&str>, input: Value) -> sikemux_wire::chat::ToolRow {
    let mut raw = json!({ "toolCallId": "t1", "title": title, "rawInput": input });
    if let Some(kind) = kind {
        raw["kind"] = json!(kind);
    }
    *tools::call(raw.as_object().unwrap()).row
}

fn titled(title: &str) -> sikemux_wire::chat::ToolRow {
    row_of(title, None, json!({}))
}

#[test]
fn an_mcp_call_is_named_for_its_server_and_reads_its_tool_as_words() {
    let row = row_of(
        "mcp__linear__create_issue",
        Some("fetch_custom"),
        json!({ "title": "Rows use underscores", "body": "a\nb" }),
    );
    assert_eq!(row.action, sikemux_wire::chat::ToolAction::Mcp);
    assert_eq!(row.server.as_deref(), Some("linear"));
    assert_eq!(row.target, "create issue");
    assert_eq!(row.detail.as_deref(), Some("Rows use underscores"));
    assert_eq!(titled("mcp__server__nested__tool").target, "nested  tool");
    assert_eq!(titled("mcp__half").server, None);
}

#[test]
fn a_running_call_says_what_it_does_while_that_fits_its_line() {
    assert_eq!(
        titled("mcp__github__list_pulls\nmore").activity.as_deref(),
        Some("list pulls")
    );
    assert_eq!(titled("   ").activity, None);
    assert_eq!(titled(&"a".repeat(41)).activity, None);
    assert_eq!(titled(&"a".repeat(40)).activity, Some("a".repeat(40)));
    assert_eq!(row_of("cat x", Some("read"), json!({})).activity, None);
}

#[test]
fn an_unknown_tool_is_called_by_the_first_word_of_its_title() {
    assert_eq!(
        titled("WebSearch(query)").verb.as_deref(),
        Some("websearch")
    );
    assert_eq!(
        titled("Averyveryverylongtoolname").verb.as_deref(),
        Some("averyveryver")
    );
    assert_eq!(titled("ToolSearch").icon.as_deref(), Some("wrench"));
    assert_eq!(row_of("Read", Some("read"), json!({})).verb, None);
}

#[test]
fn a_target_shortens_a_path_and_keeps_commands_pages_and_words() {
    assert_eq!(titled("/work/demo/src/main.rs").target, "main.rs");
    assert_eq!(titled("cat src/main.rs").target, "cat src/main.rs");
    assert_eq!(
        titled("https://example.com/a/b").target,
        "https://example.com/a/b"
    );
    assert_eq!(titled("Read").target, "Read");
    assert_eq!(titled("  src/lib.rs  \nsecond").target, "lib.rs");
    assert_eq!(titled("/").target, "/");
}

#[test]
fn our_tools_say_what_they_did_to_what() {
    let navigate = |input: Value, title: &str| row_of(title, Some("other"), input);
    let title = "mcp__sikemux-tools__browser_navigate";
    let row = navigate(
        json!({ "url": "https://github.com/nodelike/sikemux" }),
        title,
    );
    assert_eq!(row.verb.as_deref(), Some("open"));
    assert_eq!(row.target, "https://github.com/nodelike/sikemux");
    assert_eq!(row.detail, None);
    assert_eq!(row.path, None);
    assert_eq!(navigate(json!({ "go": "back" }), title).target, "back");

    let click = |input: Value| row_of("mcp__sikemux-tools__browser_click", None, input);
    let row = click(json!({ "text": "Sign in", "role": "button" }));
    assert_eq!(
        (row.target.as_str(), row.detail.as_deref()),
        ("“Sign in”", Some("button"))
    );
    assert_eq!(click(json!({ "index": 0 })).target, "element 0");
    assert_eq!(click(json!({})).target, "");

    for title in [
        "sikemux-tools.browser_navigate",
        "sikemux-tools/browser_navigate",
        "browser_navigate",
    ] {
        assert_eq!(
            navigate(json!({ "url": "https://a.dev" }), title)
                .verb
                .as_deref(),
            Some("open")
        );
    }
    let nested = json!({ "server": "sikemux-tools", "tool": "browser_navigate", "arguments": { "url": "https://a.dev" } });
    assert_eq!(navigate(nested, title).target, "https://a.dev");
    assert_eq!(
        navigate(
            json!({ "url": "https://a.dev" }),
            "sikemux-tools/browser_navigate"
        )
        .path,
        None
    );
}

#[test]
fn our_tools_show_paths_by_name_lists_joined_or_counted_and_waits_as_durations() {
    let upload = row_of(
        "mcp__sikemux-tools__browser_upload",
        None,
        json!({ "paths": ["/tmp/a.png", "/tmp/b.pdf"] }),
    );
    assert_eq!(upload.target, "a.png, b.pdf");
    let act = row_of(
        "mcp__sikemux-tools__browser_act",
        None,
        json!({ "steps": [{ "action": "click" }, { "action": "press" }] }),
    );
    assert_eq!(act.target, "2-step sequence");
    assert_eq!(
        row_of("mcp__sikemux-tools__browser_screenshot", None, json!({})).target,
        ""
    );
    assert_eq!(
        row_of(
            "mcp__sikemux-tools__browser_wait",
            None,
            json!({ "ms": 4000 })
        )
        .target,
        "4s"
    );
}

#[test]
fn our_tools_say_what_they_are_doing_while_they_run() {
    let navigate = |url: String| {
        row_of(
            "mcp__sikemux-tools__browser_navigate",
            None,
            json!({ "url": url }),
        )
    };
    assert_eq!(
        navigate("https://a.dev".into()).activity.as_deref(),
        Some("open https://a.dev")
    );
    assert_eq!(
        navigate(format!("https://a.dev/{}", "x".repeat(40)))
            .activity
            .as_deref(),
        Some("open")
    );
}

#[test]
fn a_plugins_tool_wears_its_plugin() {
    let logs = row_of(
        "mcp__sikemux-tools__signoz_logs",
        None,
        json!({ "service": "api", "text": "timeout" }),
    );
    assert_eq!(
        (
            logs.verb.as_deref(),
            logs.target.as_str(),
            logs.detail.as_deref(),
            logs.plugin.as_deref()
        ),
        (
            Some("logs"),
            "api",
            Some("“timeout”"),
            Some("sikemux.signoz")
        )
    );
}

#[test]
fn a_row_opens_the_file_the_call_reported_or_the_one_its_title_names() {
    let reported = |path: &str| {
        let raw = json!({ "toolCallId": "t1", "title": "Read", "locations": [{ "path": path, "line": 12 }] });
        tools::call(raw.as_object().unwrap()).row.path.clone()
    };
    assert_eq!(reported("/a/b.ts").as_deref(), Some("/a/b.ts:12"));
    assert_eq!(titled("src/app.tsx").path.as_deref(), Some("src/app.tsx"));
    assert_eq!(titled("cat src/app.tsx").path, None);
    assert_eq!(titled("https://example.com/x").path, None);
    assert_eq!(titled("Read").path, None);
}

mod window {
    use super::*;

    /// A turn: the person's prompt, then the agent's answer of `answer`.
    fn turn(chat: &mut Chat, n: usize, answer: &str) -> (u64, u64) {
        let asked = chat.index;
        chat.prompt(json!({ "text": format!("ask {n}"), "paths": [] }));
        chat.turn_started();
        let answered = chat.index;
        chat.update(chunk(answer, &format!("a{n}")));
        chat.turn_completed(Some("end_turn"));
        (asked, answered)
    }

    fn said(row: &ChatMessage) -> String {
        row.parts
            .iter()
            .filter_map(|part| match part {
                ChatPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn sent_bytes(rows: &[ChatMessage]) -> usize {
        rows.iter()
            .map(|row| serde_json::to_vec(row).unwrap().len())
            .sum()
    }

    #[test]
    fn every_row_a_client_is_sent_names_the_event_that_opened_it() {
        let mut chat = Chat::new();
        let (asked, answered) = turn(&mut chat, 0, "Hello");
        let changes = chat.flush();
        let streamed: Vec<&ChatMessage> = changes
            .iter()
            .filter_map(|change| match change {
                ChatChange::Row { row, .. } => Some(row),
                _ => None,
            })
            .collect();
        assert_eq!(
            streamed
                .iter()
                .map(|row| row.from_event)
                .collect::<Vec<_>>(),
            vec![Some(asked), Some(answered)]
        );
        let view = chat.view();
        assert_eq!(
            view.rows
                .iter()
                .map(|row| row.from_event)
                .collect::<Vec<_>>(),
            vec![Some(asked), Some(answered)]
        );
        let event = |kind, payload| ChatEvent { kind, payload };
        let events = [
            (
                asked,
                event(
                    ChatEventKind::Prompt,
                    json!({ "text": "ask 0", "paths": [] }),
                ),
            ),
            (
                answered,
                event(
                    ChatEventKind::SessionUpdate,
                    json!({ "sessionId": ROOT, "update": chunk("Hello", "a0") }),
                ),
            ),
        ];
        let page = chat
            .fold
            .page_of(&events, PageOf::Before(u64::MAX), PAGE_BYTES, None, None);
        assert_eq!(
            page.rows
                .iter()
                .map(|row| row.from_event)
                .collect::<Vec<_>>(),
            vec![Some(asked), Some(answered)]
        );
    }

    #[test]
    fn a_client_opening_a_long_chat_is_sent_its_last_forty_turns() {
        let mut chat = Chat::new();
        for n in 0..50 {
            turn(&mut chat, n, "ok");
        }
        let view = chat.view();
        let asked = view
            .rows
            .iter()
            .filter(|row| row.role == ChatRole::User)
            .count();
        assert_eq!(asked, TAIL_TURNS);
        assert_eq!(view.older_before, view.rows[0].from_event);
        assert_eq!(view.rows[0].role, ChatRole::User);
    }

    #[test]
    fn the_window_stops_at_a_megabyte_of_the_json_it_sends_and_never_splits_a_row() {
        let mut chat = Chat::new();
        let long = "x".repeat(300 * 1024);
        for n in 0..10 {
            turn(&mut chat, n, &long);
        }
        let view = chat.view();
        let bytes = sent_bytes(&view.rows);
        assert!(bytes <= 1024 * 1024, "sent {bytes} bytes");
        let an_answer = serde_json::to_vec(view.rows.last().unwrap()).unwrap().len();
        assert!(
            bytes + an_answer > 1024 * 1024,
            "the window stopped early at {bytes} bytes"
        );
        assert_eq!(view.older_before, view.rows[0].from_event);
        assert_eq!(said(view.rows.last().unwrap()), long);
    }

    #[test]
    fn the_last_row_goes_whole_even_past_a_megabyte() {
        let mut chat = Chat::new();
        turn(&mut chat, 0, "short");
        let huge = "y".repeat(2 * 1024 * 1024);
        turn(&mut chat, 1, &huge);
        let view = chat.view();
        assert_eq!(view.rows.len(), 1);
        assert_eq!(said(&view.rows[0]), huge);
    }
}
