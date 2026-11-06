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
            let events: Vec<Value> = page
                .events
                .iter()
                .map(|entry| {
                    let mut event = json!({ "cursor": entry.seq.to_string(), "kind": entry.kind });
                    if let (Some(id), Some(object)) = (&entry.execution_id, event.as_object_mut()) {
                        object.insert("executionId".into(), id.clone().into());
                    }
                    event
                })
                .collect();
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
