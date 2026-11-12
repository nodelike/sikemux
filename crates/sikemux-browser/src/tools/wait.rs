//! Waiting on a page: the conditions an agent can name, and arriving at an address.

use super::*;

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
        "script",
        "stable",
    ] {
        if let Some(value) = source.get(key).and_then(Value::as_str) {
            condition.insert(key.into(), json!(value));
        }
    }
    if source.get("networkIdle").and_then(Value::as_bool) == Some(true) {
        condition.insert("networkIdle".into(), json!(true));
    }
    if condition.contains_key("stable") {
        if let Some(quiet) = source.get("quietMs").and_then(Value::as_u64) {
            condition.insert("quietMs".into(), json!(quiet));
        }
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
    let given_up = |failing: Vec<Value>| json!({ "met": false, "waitedMs": started.elapsed().as_millis() as u64, "failing": failing });
    let network = condition.get("networkIdle").is_some();
    let script = condition.get("script").and_then(Value::as_str);
    let stable = condition.get("stable").and_then(Value::as_str);
    let quiet = Duration::from_millis(
        condition
            .get("quietMs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_QUIET_MS),
    );
    let needs_loaded = network
        || stable.is_some()
        || ["textGone", "selectorGone", "urlGone"]
            .iter()
            .any(|key| condition.get(*key).is_some());
    let mut quiet_since: Option<Instant> = None;
    let mut last_text: Option<String> = None;
    let mut text_since = Instant::now();
    loop {
        let (tab_id, view) = match active(manager, agent_id) {
            Ok(tab) => tab,
            Err(error) => return given_up(vec![json!(error)]),
        };
        let mut failing = match ask_page(&view, "check", std::slice::from_ref(condition)).await {
            Ok(answer) => answer
                .get("failing")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Err(error) => vec![json!(error)],
        };
        if needs_loaded
            && manager
                .page(agent_id, &tab_id)
                .is_some_and(|page| page.loading)
        {
            failing.push(json!("the page is still loading"));
        }
        if let Some(script) = script {
            let left = limit.saturating_sub(started.elapsed());
            match script_holds(
                &view,
                script,
                left.clamp(CONDITION_POLL, SCRIPT_CHECK_LIMIT),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(why)) => failing.push(json!(why)),
                Err(error) => return given_up(vec![json!(error)]),
            }
        }
        if let Some(selector) = stable {
            match ask_page(&view, "extract", &[json!(selector)]).await {
                Ok(read) => {
                    let now = read.to_string();
                    if last_text.as_ref() != Some(&now) {
                        last_text = Some(now);
                        text_since = Instant::now();
                    }
                    let still = text_since.elapsed();
                    if still < quiet {
                        failing.push(json!(format!(
                            "the text of {selector} changed {} ms ago",
                            still.as_millis()
                        )));
                    }
                }
                Err(error) => {
                    last_text = None;
                    failing.push(json!(error));
                }
            }
        }
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

/// A page that is loading may not answer at once, so a wait asks again rather than stall on it.
async fn ask_page(view: &Tab, function: &str, args: &[Value]) -> Result<Value, String> {
    tokio::time::timeout(PAGE_CHECK_LIMIT, call(view, function, args))
        .await
        .unwrap_or_else(|_| Err("the page did not answer yet".into()))
}

/// Whether the agent's script returns something truthy now, or what it returned
/// instead. An error that running it again cannot cure ends the wait.
async fn script_holds(view: &Tab, script: &str, limit: Duration) -> Result<Option<String>, String> {
    let answer = match run_agent_script(view, script, limit, Some(&[])).await {
        Ok(answer) => answer,
        Err(error) if error.contains("SyntaxError") => return Err(error),
        Err(error) => return Ok(Some(format!("the script failed: {error}"))),
    };
    let result = &answer["result"];
    if let Some(origin) = result["sikemuxHeldWrite"]["origin"].as_str() {
        return Err(format!(
            "a wait's script may only read the page, but it sent a request to {origin}"
        ));
    }
    let truthy = match result {
        Value::Null | Value::Bool(false) => false,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        _ => true,
    };
    if truthy {
        return Ok(None);
    }
    let shown: String = result.to_string().chars().take(200).collect();
    Ok(Some(format!("the script returned {shown}")))
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
            let limit = Duration::from_millis(asked.min(MAX_CONDITION_WAIT_MS)).min(
                Duration::from_millis(MAX_CONDITION_WAIT_MS).saturating_sub(started.elapsed()),
            );
            let mut waited = wait_for(manager, agent_id, &condition, limit).await;
            if limit < Duration::from_millis(asked) && waited["met"] == json!(false) {
                waited["note"] = json!(format!("the load took part of the {asked} ms, so the wait gave up after {} ms; wait again with browser wait", limit.as_millis()));
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
        result["note"] = json!("the page loaded but shows no text or controls yet; a web app may still be drawing it, so wait with browser wait and a text or selector");
    }
    Ok(result)
}

pub(super) const DEFAULT_CONDITION_WAIT_MS: u64 = 10_000;

pub(super) const CONDITION_POLL: Duration = Duration::from_millis(200);

pub(super) const NETWORK_QUIET: Duration = Duration::from_millis(500);

const DEFAULT_QUIET_MS: u64 = 1_000;

const PAGE_CHECK_LIMIT: Duration = Duration::from_secs(2);

/// How long one try of a wait's script may run before the wait looks again.
const SCRIPT_CHECK_LIMIT: Duration = Duration::from_secs(5);
