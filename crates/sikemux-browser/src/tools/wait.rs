//! Waiting on a page: the conditions an agent can name, and arriving at an address.

use super::*;

pub(super) fn same_document(current: &str, wanted: &str) -> bool {
    let without_fragment = |url: &str| url.split('#').next().unwrap_or(url).to_owned();
    wanted.contains('#') && without_fragment(current) == without_fragment(wanted)
}

/// The conditions a wait names, or None when it names only a time.
pub(super) fn wait_condition(source: &Value) -> Option<Value> {
    let mut condition = serde_json::Map::new();
    for key in [
        "text",
        "textGone",
        "selector",
        "selectorGone",
        "url",
        "urlGone",
    ] {
        if let Some(value) = source.get(key).and_then(Value::as_str) {
            condition.insert(key.into(), json!(value));
        }
    }
    if source.get("networkIdle").and_then(Value::as_bool) == Some(true) {
        condition.insert("networkIdle".into(), json!(true));
    }
    (!condition.is_empty()).then_some(Value::Object(condition))
}

/// Polls until every condition holds or `limit` passes, and says which ones
/// still failed when it gave up.
pub(super) async fn wait_for(
    manager: &BrowserManager,
    agent_id: &str,
    condition: &Value,
    limit: Duration,
) -> Value {
    let started = Instant::now();
    let network = condition.get("networkIdle").is_some();
    let mut quiet_since: Option<Instant> = None;
    loop {
        let failing = match active(manager, agent_id) {
            Err(error) => {
                return json!({ "met": false, "waitedMs": started.elapsed().as_millis() as u64, "failing": [error] })
            }
            Ok((tab_id, view)) => {
                if manager
                    .page(agent_id, &tab_id)
                    .is_some_and(|page| page.loading)
                {
                    vec![json!("the page is still loading")]
                } else {
                    let mut failing =
                        match call(&view, "check", std::slice::from_ref(condition)).await {
                            Ok(answer) => answer
                                .get("failing")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default(),
                            Err(error) => vec![json!(error)],
                        };
                    if network {
                        match pending_requests(&view).await {
                            Some(0) => {
                                let since = *quiet_since.get_or_insert_with(Instant::now);
                                if since.elapsed() < NETWORK_QUIET {
                                    failing.push(json!("the network has only just gone quiet"));
                                }
                            }
                            Some(open) => {
                                quiet_since = None;
                                failing.push(json!(format!(
                                    "{open} fetch or XHR calls are still waiting on an answer"
                                )));
                            }
                            None => {}
                        }
                    }
                    failing
                }
            }
        };
        let waited = started.elapsed().as_millis() as u64;
        if failing.is_empty() {
            return json!({ "met": true, "waitedMs": waited });
        }
        if started.elapsed() >= limit {
            return json!({ "met": false, "waitedMs": waited, "failing": failing });
        }
        tokio::time::sleep(CONDITION_POLL).await;
    }
}

pub(super) async fn pending_requests(view: &Tab) -> Option<u64> {
    read_records(view, "pending", &[])
        .await
        .ok()
        .and_then(|count| count.as_u64())
}

/// What a navigation or reload ends with: any wait it asked for, the page,
/// and why the page is not what was asked for when the load failed or left
/// nothing to read yet.
pub(super) async fn arrived(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: &str,
    params: &Value,
    started: Instant,
) -> Result<Value, String> {
    let waited = match params.get("waitFor").and_then(wait_condition) {
        Some(condition) => {
            let asked = params["waitFor"]
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS);
            let limit = Duration::from_millis(asked.min(MAX_BLOCKING_MS))
                .min(Duration::from_millis(MAX_BLOCKING_MS).saturating_sub(started.elapsed()));
            let mut waited = wait_for(manager, agent_id, &condition, limit).await;
            if limit < Duration::from_millis(asked) && waited["met"] == json!(false) {
                waited["note"] = json!(format!("the load took part of the {asked} ms, so the wait gave up after {} ms; wait again with browser_wait", limit.as_millis()));
            }
            Some(waited)
        }
        None => None,
    };
    let mut result = report(manager, agent_id, params).await?;
    if let Some(waited) = waited {
        result = merge(waited, result)?;
    }
    if let Some(load) = manager.documents(tab_id, true).last() {
        if let Some(error) = &load.error {
            result["loadError"] = json!(format!("{} did not load: {error}", load.url));
        }
    }
    let blank = |key: &str| result.get(key).and_then(Value::as_str) == Some("");
    if blank("elements")
        && blank("text")
        && result.get("loadError").is_none()
        && result["url"] != json!(BLANK_URL)
    {
        result["note"] = json!("the page loaded but shows no text or controls yet; a web app may still be drawing it, so wait with browser_wait and a text or selector");
    }
    Ok(result)
}

pub(super) const DEFAULT_CONDITION_WAIT_MS: u64 = 10_000;

pub(super) const CONDITION_POLL: Duration = Duration::from_millis(200);

pub(super) const NETWORK_QUIET: Duration = Duration::from_millis(500);
