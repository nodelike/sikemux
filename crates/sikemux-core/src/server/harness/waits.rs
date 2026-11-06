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
    let cursor = required_text(params, "cursor")?;
    let after = cursor
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| cursor.parse::<u64>().ok())
        .flatten()
        .ok_or("Event cursor is invalid; take a fresh one from workspace_inspect")?;
    let timeout = integer(params, "timeoutMs", MAX_WAIT_MS, MAX_WAIT_MS, 0)?;
    let execution_id = text(params, "executionId")?;
    let harness = &core.harness;
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
