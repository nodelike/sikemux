//! The tools that wait: `events_wait`, and agents waiting on their plugins to change.

use super::*;

pub(crate) struct WaitSlot<'a>(pub(super) &'a AtomicUsize);

impl Drop for WaitSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) async fn events_wait(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    if params.get("until").is_some() {
        return run_until(core, request).await;
    }
    let cursor = match params.get("cursor") {
        Some(Value::Null) => None,
        Some(Value::String(cursor)) if cursor.trim().is_empty() => None,
        _ => text(params, "cursor")?,
    };
    let harness = &core.harness;
    let after = match cursor {
        Some(cursor) => cursor
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| cursor.parse::<u64>().ok())
            .flatten()
            .ok_or("Event cursor is invalid; take a fresh one from workspace_inspect")?,
        None => harness.journal_cursor(&request.project),
    };
    let execution_id = text(params, "executionId")?;
    let over_already = cursor.is_none()
        && execution_id.is_some_and(|id| {
            harness
                .read(|state| state.run(&request.project, id))
                .is_ok_and(|run| !run.status.is_active())
        });
    let timeout = match over_already {
        true => 0,
        false => integer(params, "timeoutMs", MAX_WAIT_MS, MAX_WAIT_MS, 0)?,
    };
    let _slot = harness.wait_slot()?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    loop {
        let mut changed = harness.changes.subscribe();
        let mut page =
            harness.read(|state| state.journals.since(&request.project, after, execution_id))?;
        page.events
            .retain(|entry| !entry.kind.starts_with(AGENT_EVENT));
        if !page.events.is_empty() || page.truncated || Instant::now() >= deadline {
            let mut events: Vec<Value> = Vec::new();
            for entry in &page.events {
                if let Some(last) = events.last_mut().filter(|last| {
                    last["kind"] == entry.kind.as_str()
                        && last.get("executionId").and_then(Value::as_str)
                            == entry.execution_id.as_deref()
                }) {
                    last["cursor"] = entry.seq.to_string().into();
                    last["count"] = (last["count"].as_u64().unwrap_or(1) + 1).into();
                    continue;
                }
                let mut event = json!({ "cursor": entry.seq.to_string(), "kind": entry.kind });
                if let (Some(id), Some(object)) = (&entry.execution_id, event.as_object_mut()) {
                    object.insert("executionId".into(), id.clone().into());
                }
                events.push(event);
            }
            return Ok(json!({
                "events": events,
                "cursor": page.cursor.to_string(),
                "truncated": page.truncated,
            }));
        }
        tokio::select! {
            _ = changed.changed() => {}
            _ = tokio::time::sleep_until(deadline) => {}
        }
    }
}

/// Holds the wait until one run exits, gets ready, or prints the text `until` names, so an
/// agent need not wake on every chunk of its output.
async fn run_until(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let params = &request.params;
    let until = required_text(params, "until")?;
    let execution_id =
        text(params, "executionId")?.ok_or("until waits on one run; pass its executionId too")?;
    let timeout = integer(params, "timeoutMs", MAX_WAIT_MS, MAX_WAIT_MS, 0)?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let project = request.project.as_str();
    let harness = &core.harness;
    let run = harness.read(|state| state.run(project, execution_id))?;
    let pattern = match until {
        "exit" => None,
        "ready" => Some(run.ready_when.clone().ok_or(
            "this run was started without readyWhen; pass the text that means ready as until",
        )?),
        text => Some(text.to_owned()),
    };
    let _slot = harness.wait_slot()?;
    let met = match &pattern {
        Some(pattern) => output_appears(core, project, execution_id, pattern, deadline).await?,
        None => loop {
            let mut changed = harness.changes.subscribe();
            let run = harness.read(|state| state.run(project, execution_id))?;
            if !run.status.is_active() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            tokio::select! {
                _ = changed.changed() => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        },
    };
    let run = harness.read(|state| state.run(project, execution_id))?;
    let mut fields = json!({ "met": met });
    if let Some(tail) = run_tail(core, &run).await {
        fields["tail"] = tail.into();
    }
    if !met {
        fields["note"] = match run.status {
            RunStatus::AwaitingTrust => AWAITING_TRUST_NOTE.into(),
            status if status.is_active() => {
                "Not yet; call events_wait with the same until again.".into()
            }
            _ => "The run ended first.".into(),
        };
    }
    Ok(with_fields(&run, fields))
}

/// The last lines a run printed, as plain text.
pub(crate) async fn run_tail(core: &Core, run: &Run) -> Option<String> {
    let query = OutputQuery {
        cursor: 0,
        limit: 4096,
        tail: Some(RUN_TAIL_LINES),
        search: None,
        context: 0,
        plain: true,
    };
    let page = read_output(core, run.pty_id?, query).await.ok()?;
    let tail = String::from_utf8_lossy(&page.bytes).trim_end().to_owned();
    (!tail.is_empty()).then_some(tail)
}

/// Whether the run has printed `text`.
pub(crate) async fn run_printed(core: &Core, run: &Run, text: &str) -> bool {
    let Some(pty_id) = run.pty_id else {
        return false;
    };
    let query = OutputQuery {
        cursor: 0,
        limit: 4096,
        tail: Some(1),
        search: Some(text.to_owned()),
        context: 0,
        plain: false,
    };
    read_output(core, pty_id, query)
        .await
        .is_ok_and(|page| page.matches.unwrap_or(0) > 0)
}

/// Answers with how many times plugins have changed, once that is no longer
/// `seen` or the wait runs out.
pub(super) async fn plugins_wait(harness: &Harness, params: &Value) -> Result<Value, String> {
    let seen = params.get("seen").and_then(Value::as_u64);
    if harness.plugin_waits.fetch_add(1, Ordering::AcqRel) >= MAX_PLUGIN_WAITS {
        harness.plugin_waits.fetch_sub(1, Ordering::AcqRel);
        return Err("Too many plugin waits".into());
    }
    let _slot = WaitSlot(&harness.plugin_waits);
    let mut changes = harness.plugins.subscribe();
    if seen == Some(*changes.borrow_and_update()) {
        let changed = tokio::time::timeout(PLUGIN_WAIT, changes.changed()).await;
        if matches!(changed, Ok(Ok(()))) {
            tokio::time::sleep(PLUGIN_SETTLE).await;
        }
    }
    let version = *harness.plugins.borrow();
    Ok(json!({ "version": version }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 70 calls since 2026-11-03 failed with "cursor must be nonempty text": a note from
    /// `process_start` tells agents to wait on its `executionId`, and they had no cursor to pass.
    #[tokio::test]
    async fn a_wait_with_no_cursor_starts_from_now() {
        let core = Core::new(Default::default(), None).unwrap();
        let project = tempfile::tempdir().unwrap();
        let project = project.path().to_string_lossy().into_owned();
        core.harness.change(|state| {
            state
                .journals
                .append(&project, "process.running", Some("a"), None);
        });
        let now = core.harness.journal_cursor(&project).to_string();
        let asking = |params: Value| HarnessRequest {
            id: "wait".to_owned(),
            project: project.clone(),
            agent_id: None,
            method: "events.wait".to_owned(),
            params,
        };
        for params in [
            json!({ "timeoutMs": 0 }),
            json!({ "cursor": "", "timeoutMs": 0 }),
            json!({ "cursor": null, "timeoutMs": 0 }),
        ] {
            let answer = events_wait(&core, &asking(params)).await.unwrap();
            assert_eq!(answer["events"], json!([]));
            assert_eq!(answer["cursor"], now);
        }
        let earlier = json!({ "cursor": "0", "timeoutMs": 0 });
        let answer = events_wait(&core, &asking(earlier)).await.unwrap();
        assert_eq!(answer["events"].as_array().unwrap().len(), 1);
        assert!(events_wait(&core, &asking(json!({ "cursor": 5 })))
            .await
            .is_err());
    }
}
