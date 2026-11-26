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

fn has_dom_conditions(condition: &Value) -> bool {
    [
        "text",
        "textGone",
        "selector",
        "selectorGone",
        "url",
        "urlGone",
        "stable",
    ]
    .iter()
    .any(|key| condition.get(*key).is_some())
}

/// Waits until every condition holds or `limit` passes, and says which ones
/// still failed when it gave up. Uses an in-page MutationObserver for DOM/URL/stable
/// conditions instead of polling from Rust.
pub(super) async fn wait_for(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: &str,
    condition: &Value,
    limit: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    let given_up = |failing: Vec<Value>| {
        Ok(
            json!({ "met": false, "waitedMs": started.elapsed().as_millis() as u64, "failing": failing }),
        )
    };
    if let Some(dialog) = manager.dialog(tab_id) {
        return Err(native::dialog_blocked_error(&dialog));
    }
    let network = condition.get("networkIdle").is_some();
    let script = condition.get("script").and_then(Value::as_str);
    let stable = condition.get("stable").and_then(Value::as_str);
    let dom_only = has_dom_conditions(condition) && !network && script.is_none();
    let needs_loaded = stable.is_some()
        || ["textGone", "selectorGone", "urlGone"]
            .iter()
            .any(|key| condition.get(*key).is_some());

    if dom_only {
        return wait_dom_event_driven(
            manager,
            agent_id,
            tab_id,
            condition,
            limit,
            needs_loaded,
            started,
        )
        .await;
    }

    let quiet = Duration::from_millis(
        condition
            .get("quietMs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_QUIET_MS),
    );
    let mut network_tracker = NetworkIdleTracker::default();
    let mut last_text: Option<String> = None;
    let mut text_since = Instant::now();

    loop {
        if let Some(dialog) = manager.dialog(tab_id) {
            return Err(native::dialog_blocked_error(&dialog));
        }
        if needs_loaded
            && manager
                .page(agent_id, tab_id)
                .is_some_and(|page| page.loading)
        {
            let left = limit.saturating_sub(started.elapsed());
            if left.is_zero() {
                return given_up(vec![json!("the page is still loading")]);
            }
            let _ = manager.wait_until_loaded(agent_id, tab_id, left).await;
            if let Some(dialog) = manager.dialog(tab_id) {
                return Err(native::dialog_blocked_error(&dialog));
            }
        }

        let (_, view) = match target_tab(manager, agent_id, Some(tab_id)) {
            Ok(tab) => tab,
            Err(error) if manager.dialog(tab_id).is_some() => return Err(error),
            Err(error) => return given_up(vec![json!(error)]),
        };

        // If DOM conditions are combined with networkIdle/script, let the MutationObserver
        // satisfy DOM conditions first without 200ms polling.
        if has_dom_conditions(condition) && stable.is_none() {
            let left = limit.saturating_sub(started.elapsed());
            if !left.is_zero() {
                let watch = tokio::select! {
                    biased;
                    dialog = manager.wait_for_dialog(tab_id) => {
                        return Err(native::dialog_blocked_error(&dialog));
                    }
                    watched = watch_once(&view, condition, left) => watched,
                };
                if let Ok(Some(watched)) = watch {
                    if watched.get("met").and_then(Value::as_bool) == Some(false)
                        && started.elapsed() >= limit
                    {
                        return Ok(watched);
                    }
                }
            }
        }

        let mut failing = match ask_page(&view, "check", std::slice::from_ref(condition)).await {
            Ok(answer) => answer
                .get("failing")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Err(error) => vec![json!(error)],
        };
        if let Some(dialog) = manager.dialog(tab_id) {
            return Err(native::dialog_blocked_error(&dialog));
        }
        if needs_loaded
            && manager
                .page(agent_id, tab_id)
                .is_some_and(|page| page.loading)
        {
            failing.push(json!("the page is still loading"));
        }
        if let Some(script) = script {
            let left = limit.saturating_sub(started.elapsed());
            match script_holds(
                manager,
                agent_id,
                tab_id,
                &view,
                script,
                left.clamp(CONDITION_POLL, SCRIPT_CHECK_LIMIT),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(why)) => failing.push(json!(why)),
                Err(error) if manager.dialog(tab_id).is_some() => return Err(error),
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
        let mut network_note = None;
        if network {
            let page_loading = manager
                .page(agent_id, tab_id)
                .is_some_and(|page| page.loading);
            let pending = pending_requests(&view).await;
            match network_tracker.evaluate(
                Instant::now(),
                page_loading,
                pending.as_ref(),
                NETWORK_QUIET,
                EXTERNAL_SETTLE_GRACE,
            ) {
                NetworkIdleState::Idle { note } => {
                    network_note = note;
                }
                NetworkIdleState::Busy { failing: why } => {
                    failing.push(json!(why));
                }
            }
        }
        let waited = started.elapsed().as_millis() as u64;
        if failing.is_empty() {
            let mut answer = json!({ "met": true, "waitedMs": waited });
            if let Some(note) = network_note {
                put(&mut answer, "note", json!(note));
            }
            return Ok(answer);
        }
        if started.elapsed() >= limit {
            return Ok(json!({ "met": false, "waitedMs": waited, "failing": failing }));
        }
        tokio::select! {
            biased;
            dialog = manager.wait_for_dialog(tab_id) => {
                return Err(native::dialog_blocked_error(&dialog));
            }
            () = tokio::time::sleep(CONDITION_POLL) => {}
        }
    }
}

/// Runs `window.__sikemux.watch` inside the page's helper world, re-attaching if the tab
/// navigates mid-wait.
async fn wait_dom_event_driven(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: &str,
    condition: &Value,
    limit: Duration,
    needs_loaded: bool,
    started: Instant,
) -> Result<Value, String> {
    let given_up = |failing: Vec<Value>| {
        Ok(
            json!({ "met": false, "waitedMs": started.elapsed().as_millis() as u64, "failing": failing }),
        )
    };
    loop {
        if let Some(dialog) = manager.dialog(tab_id) {
            return Err(native::dialog_blocked_error(&dialog));
        }
        if needs_loaded
            && manager
                .page(agent_id, tab_id)
                .is_some_and(|page| page.loading)
        {
            let left = limit.saturating_sub(started.elapsed());
            if left.is_zero() {
                return given_up(vec![json!("the page is still loading")]);
            }
            let _ = manager.wait_until_loaded(agent_id, tab_id, left).await;
            if let Some(dialog) = manager.dialog(tab_id) {
                return Err(native::dialog_blocked_error(&dialog));
            }
        }
        let (_, view) = match target_tab(manager, agent_id, Some(tab_id)) {
            Ok(tab) => tab,
            Err(error) if manager.dialog(tab_id).is_some() => return Err(error),
            Err(error) => return given_up(vec![json!(error)]),
        };
        let left = limit.saturating_sub(started.elapsed());
        let watched = tokio::select! {
            biased;
            dialog = manager.wait_for_dialog(tab_id) => {
                return Err(native::dialog_blocked_error(&dialog));
            }
            result = watch_once(&view, condition, left) => result,
        };
        match watched {
            Ok(Some(mut answer)) => {
                let met = answer.get("met").and_then(Value::as_bool) == Some(true);
                if met
                    && needs_loaded
                    && manager
                        .page(agent_id, tab_id)
                        .is_some_and(|page| page.loading)
                {
                    if started.elapsed() >= limit {
                        return given_up(vec![json!("the page is still loading")]);
                    }
                    continue;
                }
                put(
                    &mut answer,
                    "waitedMs",
                    json!(started.elapsed().as_millis() as u64),
                );
                return Ok(answer);
            }
            Ok(None) => {
                if let Some(dialog) = manager.dialog(tab_id) {
                    return Err(native::dialog_blocked_error(&dialog));
                }
                let failing = match ask_page(&view, "check", std::slice::from_ref(condition)).await
                {
                    Ok(answer) => answer
                        .get("failing")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    Err(error) => vec![json!(error)],
                };
                return given_up(failing);
            }
            Err(error) => {
                if let Some(dialog) = manager.dialog(tab_id) {
                    return Err(native::dialog_blocked_error(&dialog));
                }
                if started.elapsed() >= limit {
                    return given_up(vec![json!(error)]);
                }
                // A navigation may have dropped the helper promise; wait for load or a short beat.
                let left = limit.saturating_sub(started.elapsed());
                if manager
                    .page(agent_id, tab_id)
                    .is_some_and(|page| page.loading)
                {
                    let _ = manager.wait_until_loaded(agent_id, tab_id, left).await;
                } else {
                    tokio::select! {
                        biased;
                        dialog = manager.wait_for_dialog(tab_id) => {
                            return Err(native::dialog_blocked_error(&dialog));
                        }
                        () = tokio::time::sleep(Duration::from_millis(50).min(left)) => {}
                    }
                }
            }
        }
    }
}

async fn watch_once(
    view: &Tab,
    condition: &Value,
    limit: Duration,
) -> Result<Option<Value>, String> {
    let cond_json = serde_json::to_string(condition).unwrap_or_else(|_| "{}".into());
    let ms = limit.as_millis() as u64;
    let expr = format!(
        "JSON.stringify((await (async () => {{ try {{ return {{ ok: await window.__sikemux.watch({cond_json}, {ms}) }}; }} catch (error) {{ return {{ error: String((error && error.message) || error) }}; }} }})()))"
    );
    let wall = limit + Duration::from_millis(500);
    let Some(raw) = wait_page_helper(view, &expr, wall).await? else {
        return Ok(None);
    };
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|_| "the page returned an unreadable answer".to_string())?;
    unwrap_answer(parsed).map(Some)
}

/// A page that is loading may not answer at once, so a wait asks again rather than stall on it.
async fn ask_page(view: &Tab, function: &str, args: &[Value]) -> Result<Value, String> {
    tokio::time::timeout(PAGE_CHECK_LIMIT, call(view, function, args))
        .await
        .unwrap_or_else(|_| Err("the page did not answer yet".into()))
}

/// Whether the agent's script returns something truthy now, or what it returned
/// instead. An error that running it again cannot cure ends the wait.
async fn script_holds(
    manager: &BrowserManager,
    agent_id: &str,
    tab_id: &str,
    view: &Tab,
    script: &str,
    limit: Duration,
) -> Result<Option<String>, String> {
    let answer = match run_agent_script(
        view,
        script,
        limit,
        Some(&[]),
        Some((manager, agent_id, tab_id)),
    )
    .await
    {
        Ok(answer) => answer,
        Err(error) if error.contains("SyntaxError") || manager.dialog(tab_id).is_some() => {
            return Err(error);
        }
        Err(error) => return Ok(Some(format!("the script failed: {error}"))),
    };
    let result = answer.get("result").unwrap_or(&Value::Null);
    if let Some(origin) = result
        .pointer("/sikemuxHeldWrite/origin")
        .and_then(Value::as_str)
    {
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct InflightCall {
    pub(super) method: String,
    pub(super) url: String,
    pub(super) age_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PendingNetwork {
    pub(super) ready_state: String,
    pub(super) same_origin: Vec<InflightCall>,
    pub(super) external: Vec<InflightCall>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NetworkIdleState {
    Idle { note: Option<String> },
    Busy { failing: String },
}

#[derive(Default)]
pub(super) struct NetworkIdleTracker {
    quiet_since: Option<Instant>,
    main_quiet_since: Option<Instant>,
}

impl NetworkIdleTracker {
    pub(super) fn evaluate(
        &mut self,
        now: Instant,
        page_loading: bool,
        pending: Option<&PendingNetwork>,
        quiet: Duration,
        external_grace: Duration,
    ) -> NetworkIdleState {
        let Some(net) = pending else {
            if page_loading {
                self.quiet_since = None;
                self.main_quiet_since = None;
                return NetworkIdleState::Busy {
                    failing: "the page is still loading".into(),
                };
            }
            let since = *self.quiet_since.get_or_insert(now);
            return if now.saturating_duration_since(since) >= quiet {
                NetworkIdleState::Idle { note: None }
            } else {
                NetworkIdleState::Busy {
                    failing: "the network has only just gone quiet".into(),
                }
            };
        };

        let interactive = matches!(net.ready_state.as_str(), "interactive" | "complete");
        let fully_clear = !page_loading && net.same_origin.is_empty() && net.external.is_empty();
        if fully_clear {
            let since = *self.quiet_since.get_or_insert(now);
            self.main_quiet_since.get_or_insert(now);
            return if now.saturating_duration_since(since) >= quiet {
                NetworkIdleState::Idle { note: None }
            } else {
                NetworkIdleState::Busy {
                    failing: "the network has only just gone quiet".into(),
                }
            };
        }

        self.quiet_since = None;
        if interactive && net.same_origin.is_empty() {
            let main_since = *self.main_quiet_since.get_or_insert(now);
            if now.saturating_duration_since(main_since) >= external_grace {
                let note = if !net.external.is_empty() {
                    format!(
                        "main-origin network settled; ignored slow external requests: {}",
                        format_inflight(&net.external)
                    )
                } else {
                    "page is interactive and main-origin network settled; slow subresources still loading"
                        .into()
                };
                return NetworkIdleState::Idle { note: Some(note) };
            }
        } else {
            self.main_quiet_since = None;
        }

        NetworkIdleState::Busy {
            failing: describe_busy(page_loading, net),
        }
    }
}

fn format_inflight(calls: &[InflightCall]) -> String {
    const MAX_SHOWN: usize = 5;
    const MAX_URL: usize = 120;
    let mut parts: Vec<String> = calls
        .iter()
        .take(MAX_SHOWN)
        .map(|call| {
            let url = if call.url.chars().count() > MAX_URL {
                let head: String = call.url.chars().take(MAX_URL).collect();
                format!("{head}…")
            } else {
                call.url.clone()
            };
            format!("{} {} ({}ms)", call.method, url, call.age_ms)
        })
        .collect();
    if calls.len() > MAX_SHOWN {
        parts.push(format!("+{} more", calls.len() - MAX_SHOWN));
    }
    parts.join(", ")
}

fn describe_busy(page_loading: bool, net: &PendingNetwork) -> String {
    if !net.same_origin.is_empty() {
        return format!(
            "{} same-origin request(s) still pending: {}",
            net.same_origin.len(),
            format_inflight(&net.same_origin)
        );
    }
    if !net.external.is_empty() {
        return format!(
            "{} external request(s) still pending: {}",
            net.external.len(),
            format_inflight(&net.external)
        );
    }
    if page_loading {
        let state = if net.ready_state.is_empty() {
            "loading"
        } else {
            net.ready_state.as_str()
        };
        return format!("the page is still loading (readyState: {state})");
    }
    "the network has only just gone quiet".into()
}

fn parse_inflight_list(value: Option<&Value>) -> Vec<InflightCall> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let url = item.get("url").and_then(Value::as_str)?.to_owned();
                    let method = item
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or("GET")
                        .to_owned();
                    let age_ms = item.get("ageMs").and_then(Value::as_u64).unwrap_or(0);
                    Some(InflightCall {
                        method,
                        url,
                        age_ms,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_pending_network(value: &Value) -> Option<PendingNetwork> {
    let obj = value.as_object()?;
    Some(PendingNetwork {
        ready_state: obj
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        same_origin: parse_inflight_list(obj.get("sameOrigin")),
        external: parse_inflight_list(obj.get("external")),
    })
}

pub(super) async fn pending_requests(view: &Tab) -> Option<PendingNetwork> {
    let value = tokio::time::timeout(PAGE_CHECK_LIMIT, read_records(view, "pending", &[]))
        .await
        .ok()?
        .ok()?;
    parse_pending_network(&value)
}

fn latest_navigation_failure(
    manager: &BrowserManager,
    tab_id: &str,
    requested_url: Option<&str>,
) -> Option<String> {
    let load = manager.documents(tab_id, true).into_iter().last()?;
    if !load.is_navigation_failure() {
        return None;
    }
    let target = if !load.url.is_empty() && load.url != BLANK_URL {
        load.url.as_str()
    } else {
        requested_url.unwrap_or("the page")
    };
    let detail = load.error.as_deref().unwrap_or("navigation failed");
    Some(format!("{target} did not load: {detail}"))
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
    include_tabs: bool,
    requested_url: Option<&str>,
) -> Result<Value, String> {
    if let Some(error) = latest_navigation_failure(manager, tab_id, requested_url) {
        return Err(error);
    }
    let waited = match params.get("waitFor").and_then(wait_condition) {
        Some(condition) => {
            let asked = params
                .pointer("/waitFor/timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS);
            let limit = Duration::from_millis(asked.min(MAX_CONDITION_WAIT_MS)).min(
                Duration::from_millis(MAX_CONDITION_WAIT_MS).saturating_sub(started.elapsed()),
            );
            let mut waited = wait_for(manager, agent_id, tab_id, &condition, limit).await?;
            if limit < Duration::from_millis(asked)
                && waited.get("met").and_then(Value::as_bool) == Some(false)
            {
                put(&mut waited, "note", json!(format!("the load took part of the {asked} ms, so the wait gave up after {} ms; wait again with browser_wait", limit.as_millis())));
            }
            Some(waited)
        }
        None => None,
    };
    if let Some(error) = latest_navigation_failure(manager, tab_id, requested_url) {
        return Err(error);
    }
    let mut result = report(manager, agent_id, tab_id, params, include_tabs).await?;
    if let Some(waited) = waited {
        result = merge(waited, result)?;
    }
    if let Some(load) = manager.documents(tab_id, true).last() {
        if let Some(error) = &load.error {
            put(
                &mut result,
                "loadError",
                json!(format!("{} did not load: {error}", load.url)),
            );
        }
    }
    let blank = |key: &str| result.get(key).and_then(Value::as_str) == Some("");
    if blank("elements")
        && blank("text")
        && result.get("loadError").is_none()
        && result.get("url").and_then(Value::as_str) != Some(BLANK_URL)
    {
        put(&mut result, "note", json!("the page loaded but shows no text or controls yet; a web app may still be drawing it, so wait with browser_wait and a text or selector"));
    }
    Ok(result)
}

pub(super) const DEFAULT_CONDITION_WAIT_MS: u64 = 10_000;

pub(super) const CONDITION_POLL: Duration = Duration::from_millis(100);

pub(super) const NETWORK_QUIET: Duration = Duration::from_millis(500);

pub(super) const EXTERNAL_SETTLE_GRACE: Duration = Duration::from_secs(3);

pub(super) const NAV_NETWORK_IDLE_LOAD_CAP: Duration = Duration::from_secs(5);

const DEFAULT_QUIET_MS: u64 = 1_000;

const PAGE_CHECK_LIMIT: Duration = Duration::from_secs(2);

/// How long one try of a wait's script may run before the wait looks again.
const SCRIPT_CHECK_LIMIT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_idle_settles_on_slow_external_assets_once_main_origin_is_quiet() {
        let mut tracker = NetworkIdleTracker::default();
        let t0 = Instant::now();
        let hanging_external = PendingNetwork {
            ready_state: "interactive".into(),
            same_origin: vec![],
            external: vec![InflightCall {
                method: "GET".into(),
                url: "https://analytics.thirdparty.test/pixel.js".into(),
                age_ms: 1200,
            }],
        };

        let first = tracker.evaluate(
            t0,
            true,
            Some(&hanging_external),
            Duration::from_millis(500),
            Duration::from_secs(3),
        );
        match first {
            NetworkIdleState::Busy { failing } => {
                assert!(failing.contains("external request(s) still pending"));
                assert!(failing.contains("https://analytics.thirdparty.test/pixel.js"));
            }
            other => panic!("expected Busy before grace period, got {other:?}"),
        }

        let settled = tracker.evaluate(
            t0 + Duration::from_secs(3),
            true,
            Some(&hanging_external),
            Duration::from_millis(500),
            Duration::from_secs(3),
        );
        match settled {
            NetworkIdleState::Idle { note: Some(note) } => {
                assert!(note.contains("main-origin network settled"));
                assert!(note.contains("https://analytics.thirdparty.test/pixel.js"));
            }
            other => panic!("expected Idle with external diagnostic note, got {other:?}"),
        }
    }

    #[test]
    fn network_idle_reports_pending_same_origin_urls_and_does_not_settle_early() {
        let mut tracker = NetworkIdleTracker::default();
        let t0 = Instant::now();
        let slow_api = PendingNetwork {
            ready_state: "complete".into(),
            same_origin: vec![InflightCall {
                method: "POST".into(),
                url: "http://t1.localhost:8000/api/search".into(),
                age_ms: 4200,
            }],
            external: vec![],
        };

        let state = tracker.evaluate(
            t0 + Duration::from_secs(5),
            false,
            Some(&slow_api),
            Duration::from_millis(500),
            Duration::from_secs(3),
        );
        match state {
            NetworkIdleState::Busy { failing } => {
                assert!(failing.contains("1 same-origin request(s) still pending"));
                assert!(failing.contains("POST http://t1.localhost:8000/api/search (4200ms)"));
            }
            other => panic!("expected Busy for same-origin pending call, got {other:?}"),
        }
    }

    #[test]
    fn network_idle_settles_quickly_when_fully_clear() {
        let mut tracker = NetworkIdleTracker::default();
        let t0 = Instant::now();
        let clear = PendingNetwork {
            ready_state: "complete".into(),
            same_origin: vec![],
            external: vec![],
        };

        assert!(matches!(
            tracker.evaluate(
                t0,
                false,
                Some(&clear),
                Duration::from_millis(500),
                Duration::from_secs(3)
            ),
            NetworkIdleState::Busy { .. }
        ));
        assert_eq!(
            tracker.evaluate(
                t0 + Duration::from_millis(500),
                false,
                Some(&clear),
                Duration::from_millis(500),
                Duration::from_secs(3)
            ),
            NetworkIdleState::Idle { note: None }
        );
    }
}
