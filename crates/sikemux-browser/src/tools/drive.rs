//! Host-side sandboxed JS orchestration (`browser_drive`).
//!
//! Runs an agent-supplied async script in an isolated QuickJS runtime with strict
//! timeout and memory limits. Every browser action routes strictly through Sikemux's
//! gated `tools::run` primitives so `CheckWebWrite`, `person::leave_be`, and trusted
//! OS input are enforced with zero duplicated security logic.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, CaughtError, Function, Promise};
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use tokio::sync::{mpsc, oneshot};

use super::*;

pub(super) const DEFAULT_DRIVE_TIMEOUT_MS: u64 = 45_000;
const DRIVE_MEMORY_LIMIT: usize = 32 * 1024 * 1024;
const DRIVE_STACK_LIMIT: usize = 512 * 1024;
const MAX_DRIVE_STEPS: usize = 200;

/// JS SDK exposed as the `browser` global inside `browser_drive`.
/// Pure JS only — no filesystem, network, env, or DOM globals exist in this runtime.
const DRIVE_SDK_JS: &str = r#"
(() => {
    const normalizeTarget = (arg, opts) => {
        if (typeof arg === "number") return { index: arg, ...(opts || {}) };
        if (typeof arg === "string") return { text: arg, ...(opts || {}) };
        return { ...(arg || {}), ...(opts || {}) };
    };
    const isGateError = (msg) => typeof msg === "string" && msg.startsWith("Waits for the person");

    const invoke = async (method, params) => {
        const optional = Boolean(params && params.optional === true);
        const clean = params ? { ...params } : {};
        delete clean.optional;
        try {
            const raw = await __sikemuxInvoke(method, JSON.stringify(clean));
            const env = JSON.parse(raw);
            if (env && env.error !== undefined) throw new Error(env.error);
            return env.ok;
        } catch (err) {
            const msg = String((err && err.message) || err);
            if (optional && !isGateError(msg)) {
                return { skipped: true, error: msg };
            }
            throw err;
        }
    };

    const makeTab = (initialId, initialMeta) => {
        let currentId = initialId || null;
        let currentUrl = (initialMeta && initialMeta.url) || "";
        let currentTitle = (initialMeta && initialMeta.title) || "";

        const callOnTab = async (method, params) => {
            const payload = { ...(params || {}) };
            if (currentId && payload.tabId === undefined) payload.tabId = currentId;
            const res = await invoke(method, payload);
            if (res && typeof res === "object") {
                if (typeof res.tabId === "string" && res.tabId) currentId = res.tabId;
                if (typeof res.url === "string") currentUrl = res.url;
                if (typeof res.title === "string") currentTitle = res.title;
            }
            return res;
        };

        return {
            get id() { return currentId; },
            get url() { return currentUrl; },
            get title() { return currentTitle; },
            goto(urlOrOpts, opts) {
                const p = typeof urlOrOpts === "string" ? { url: urlOrOpts, ...(opts || {}) } : { ...(urlOrOpts || {}) };
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.navigate", p);
            },
            click(target, opts) {
                const p = normalizeTarget(target, opts);
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.click", p);
            },
            hover(target, opts) {
                const p = normalizeTarget(target, opts);
                p.hover = true;
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.click", p);
            },
            type(first, second, third) {
                let p;
                if (typeof first === "object" && first !== null) {
                    if (typeof second === "string") {
                        p = { ...first, text: second, ...(third || {}) };
                    } else if (typeof second === "object" && second !== null) {
                        p = { ...first, ...second };
                    } else {
                        p = { ...first };
                    }
                } else if (typeof first === "number") {
                    p = { index: first, text: String(second ?? ""), ...(third || {}) };
                } else if (typeof second === "string") {
                    p = { field: String(first), text: second, ...(third || {}) };
                } else {
                    p = { text: String(first ?? ""), ...(typeof second === "object" && second !== null ? second : {}) };
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.type", p);
            },
            select(target, value, opts) {
                let p;
                if (typeof target === "object" && target !== null && value === undefined) {
                    p = { ...target };
                } else if (typeof target === "number") {
                    p = { index: target, value, ...(opts || {}) };
                } else if (typeof target === "string") {
                    p = { field: target, value, ...(opts || {}) };
                } else {
                    p = { ...(target || {}), value, ...(opts || {}) };
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.select", p);
            },
            upload(target, files, opts) {
                const toPaths = (val) => Array.isArray(val) ? val.map(String) : [String(val)];
                let p;
                if (files === undefined) {
                    if (Array.isArray(target) || typeof target === "string") {
                        p = { paths: toPaths(target) };
                    } else {
                        p = { ...(target || {}) };
                        if (p.files !== undefined && p.paths === undefined) {
                            p.paths = toPaths(p.files);
                            delete p.files;
                        }
                    }
                } else if (target == null) {
                    p = { paths: toPaths(files), ...(opts || {}) };
                } else if (typeof target === "number") {
                    p = { index: target, paths: toPaths(files), ...(opts || {}) };
                } else if (typeof target === "string") {
                    p = { selector: target, paths: toPaths(files), ...(opts || {}) };
                } else {
                    p = { ...target, paths: toPaths(files), ...(opts || {}) };
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.upload", p);
            },
            dialog(action, text, opts) {
                let p;
                if (typeof action === "object" && action !== null) {
                    p = { ...action };
                } else {
                    const accept =
                        action === true ||
                        action === "accept" ||
                        action === "ok";
                    const dismiss =
                        action === false ||
                        action === "dismiss" ||
                        action === "cancel";
                    if (!accept && !dismiss) {
                        throw new Error('dialog action must be "accept", "dismiss", true, or false');
                    }
                    p = { accept };
                    if (typeof text === "string") p.text = text;
                    else if (typeof text === "object" && text !== null) Object.assign(p, text);
                    if (opts && typeof opts === "object") Object.assign(p, opts);
                }
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.dialog", p);
            },
            download(url, opts) {
                const p = typeof url === "string" ? { url, ...(opts || {}) } : { ...(url || {}) };
                return callOnTab("browser.download", p);
            },
            press(key, opts) {
                const p = { key, ...(opts || {}) };
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.press", p);
            },
            scroll(arg) {
                let p;
                if (typeof arg === "number") p = { deltaY: arg };
                else if (arg === "top" || arg === "bottom") p = { to: arg };
                else p = { ...(arg || {}) };
                return callOnTab("browser.scroll", p);
            },
            wait(arg) {
                let p;
                if (typeof arg === "number") p = { ms: arg };
                else if (typeof arg === "string") p = { text: arg };
                else p = { ...(arg || {}) };
                if (p.report === undefined) p.report = "outcome";
                return callOnTab("browser.wait", p);
            },
            state(arg) {
                const p = typeof arg === "string" ? { selector: arg } : { ...(arg || {}) };
                return callOnTab("browser.state", p);
            },
            find(query, role) {
                const p = typeof query === "object" && query !== null ? { ...query } : { query, ...(role ? { role } : {}) };
                return callOnTab("browser.find", p);
            },
            eval(script, opts) {
                return callOnTab("browser.evaluate", { script, ...(opts || {}) });
            },
            screenshot(opts) {
                return callOnTab("browser.screenshot", opts || {});
            },
            console(opts) {
                return callOnTab("browser.console", opts || {});
            },
            network(opts) {
                return callOnTab("browser.network", opts || {});
            },
            viewport(arg) {
                const p = typeof arg === "string" ? { preset: arg } : { ...(arg || {}) };
                return callOnTab("browser.viewport", p);
            },
            close() {
                return callOnTab("browser.tab.close", {});
            },
        };
    };

    globalThis.browser = Object.freeze({
        tabs() {
            return JSON.parse(__sikemuxTabs());
        },
        async open(url, opts) {
            const p = { url, newTab: true, ...(opts || {}) };
            if (p.report === undefined) p.report = "outcome";
            const res = await invoke("browser.navigate", p);
            return makeTab(res && res.tabId, res);
        },
        tab(id) {
            return makeTab(id || null, null);
        },
    });

    globalThis.sleep = (ms) => __sikemuxSleep(Math.max(0, Number(ms) || 0));
})();
"#;

/// Methods `browser_drive` is allowed to invoke through the gated host dispatcher.
const ALLOWED_DRIVE_METHODS: &[&str] = &[
    "browser.navigate",
    "browser.state",
    "browser.find",
    "browser.click",
    "browser.type",
    "browser.select",
    "browser.press",
    "browser.drag",
    "browser.upload",
    "browser.dialog",
    "browser.download",
    "browser.viewport",
    "browser.scroll",
    "browser.evaluate",
    "browser.network",
    "browser.console",
    "browser.screenshot",
    "browser.annotate",
    "browser.wait",
    "browser.tab.close",
];

pub(crate) trait DriveBackend {
    fn call<'a>(
        &'a self,
        method: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>;

    fn tabs(&self) -> Value;
}

struct LiveBackend<'a> {
    manager: &'a BrowserManager,
    agent_id: &'a str,
    project: &'a str,
    check: &'a CheckWrite,
    allowed_sites: Value,
    upload_from: Value,
    default_tab: Option<String>,
}

impl<'a> DriveBackend for LiveBackend<'a> {
    fn call<'b>(
        &'b self,
        method: String,
        mut params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'b>> {
        if method == "browser.evaluate" {
            put(&mut params, "allowedSites", self.allowed_sites.clone());
        }
        if method == "browser.upload" {
            put(&mut params, "uploadFrom", self.upload_from.clone());
        }
        if let Some(default_tab) = self.default_tab.as_deref() {
            let opens_new = method == "browser.navigate"
                && params.get("newTab").and_then(Value::as_bool) == Some(true);
            if !opens_new && params.get("tabId").is_none() {
                put(&mut params, "tabId", json!(default_tab));
            }
        }
        Box::pin(async move {
            let tab_hint = requested_tab(&params).map(str::to_owned);
            let mut marks = Vec::new();
            if method != "browser.tab.close" {
                marks.extend(
                    self.manager
                        .mark_acting_tab(self.agent_id, tab_hint.as_deref()),
                );
            }
            let outcome = match method.as_str() {
                "browser.select" => {
                    select_option(self.manager, self.agent_id, self.check, &params).await
                }
                "browser.download" => download_asset(self.manager, self.agent_id, &params).await,
                _ => {
                    Box::pin(run(
                        self.manager,
                        self.agent_id,
                        self.project,
                        self.check,
                        &method,
                        &params,
                    ))
                    .await
                }
            };
            if method != "browser.tab.close" {
                let acted_tab = outcome
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("tabId").and_then(Value::as_str))
                    .or(tab_hint.as_deref());
                marks.extend(self.manager.mark_acting_tab(self.agent_id, acted_tab));
                self.manager.release_acting(self.agent_id, marks);
            }
            outcome
        })
    }

    fn tabs(&self) -> Value {
        let snapshot =
            self.manager
                .snapshot(self.agent_id)
                .unwrap_or_else(|_| crate::BrowserSnapshot {
                    tabs: Vec::new(),
                    active_tab_id: None,
                });
        let yours: std::collections::HashSet<String> = self
            .manager
            .agents
            .locked()
            .get(self.agent_id)
            .map(|agent| {
                snapshot
                    .tabs
                    .iter()
                    .filter(|tab| agent.strip.yours(&tab.id))
                    .map(|tab| tab.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        json!(snapshot
            .tabs
            .iter()
            .map(|tab| json!({
                "id": tab.id,
                "title": tab.title,
                "url": tab.url,
                "active": tab.active,
                "loading": tab.loading,
                "yours": yours.contains(&tab.id),
            }))
            .collect::<Vec<_>>())
    }
}

pub(super) async fn run_tool(
    manager: &BrowserManager,
    agent_id: &str,
    project: &str,
    check: &CheckWrite,
    params: &Value,
) -> Result<Value, String> {
    let script = params
        .get("script")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("script is required")?;
    let timeout_ms = params
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_DRIVE_TIMEOUT_MS)
        .clamp(100, MAX_BLOCKING_MS);
    let backend = LiveBackend {
        manager,
        agent_id,
        project,
        check,
        allowed_sites: params
            .get("allowedSites")
            .cloned()
            .unwrap_or_else(|| json!([])),
        upload_from: params
            .get("uploadFrom")
            .cloned()
            .unwrap_or_else(|| json!([project])),
        default_tab: requested_tab(params).map(str::to_owned),
    };
    Ok(run_drive(&backend, script, Duration::from_millis(timeout_ms)).await)
}

fn is_held_write_error(error: &str) -> bool {
    error.starts_with("Waits for the person")
}

fn step_summary(method: &str, params: &Value) -> String {
    let str_of = |key: &str| params.get(key).and_then(Value::as_str);
    match method {
        "browser.navigate" => {
            if let Some(url) = str_of("url") {
                format!("open {url}")
            } else if let Some(go) = str_of("go") {
                format!("go {go}")
            } else {
                "navigate".into()
            }
        }
        "browser.click" => {
            let verb = if params.get("hover").and_then(Value::as_bool) == Some(true) {
                "hover"
            } else {
                "click"
            };
            if let Some(text) = str_of("text") {
                format!("{verb} \"{text}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("{verb} {sel}")
            } else if let Some(idx) = params.get("index").and_then(Value::as_u64) {
                format!("{verb} [{idx}]")
            } else {
                verb.into()
            }
        }
        "browser.type" => {
            if let Some(field) = str_of("field") {
                format!("type into \"{field}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("type into {sel}")
            } else {
                "type".into()
            }
        }
        "browser.select" => {
            let target = str_of("field")
                .or_else(|| str_of("selector"))
                .unwrap_or("select");
            format!("select in \"{target}\"")
        }
        "browser.upload" => {
            let count = params
                .get("paths")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("upload {count} file(s)")
        }
        "browser.dialog" => {
            let verb = if params.get("accept").and_then(Value::as_bool) == Some(true) {
                "accept"
            } else {
                "dismiss"
            };
            format!("{verb} dialog")
        }
        "browser.download" => format!("download {}", str_of("url").unwrap_or("asset")),
        "browser.press" => format!("press {}", str_of("key").unwrap_or("key")),
        "browser.wait" => {
            if let Some(text) = str_of("text") {
                format!("wait for \"{text}\"")
            } else if let Some(sel) = str_of("selector") {
                format!("wait for {sel}")
            } else if let Some(url) = str_of("url") {
                format!("wait for url {url}")
            } else {
                "wait".into()
            }
        }
        _ => method.strip_prefix("browser.").unwrap_or(method).to_owned(),
    }
}

struct InvokeRequest {
    method: String,
    params: Value,
    reply: oneshot::Sender<Result<Value, String>>,
}

struct HeldWrite {
    step: usize,
    method: String,
    tab_id: Option<String>,
    summary: String,
    error: String,
}

struct InFlight<'a> {
    step: usize,
    method: String,
    tab_id: Option<String>,
    summary: String,
    reply: Option<oneshot::Sender<Result<Value, String>>>,
    future: Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>>,
}

pub(crate) async fn run_drive(
    backend: &dyn DriveBackend,
    script: &str,
    timeout: Duration,
) -> Value {
    let runtime = match AsyncRuntime::new() {
        Ok(rt) => rt,
        Err(error) => {
            return json!({
                "ok": false,
                "error": format!("could not start browser_drive runtime: {error}"),
                "completedSteps": [],
            });
        }
    };
    runtime.set_memory_limit(DRIVE_MEMORY_LIMIT).await;
    runtime.set_max_stack_size(DRIVE_STACK_LIMIT).await;

    let deadline = Instant::now() + timeout;
    let gate_aborted = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    {
        let gate_flag = Arc::clone(&gate_aborted);
        let timeout_flag = Arc::clone(&timed_out);
        runtime
            .set_interrupt_handler(Some(Box::new(move || {
                if gate_flag.load(Ordering::Acquire) {
                    return true;
                }
                if Instant::now() >= deadline {
                    timeout_flag.store(true, Ordering::Release);
                    return true;
                }
                false
            })))
            .await;
    }

    let context = match AsyncContext::full(&runtime).await {
        Ok(ctx) => ctx,
        Err(error) => {
            return json!({
                "ok": false,
                "error": format!("could not initialize browser_drive context: {error}"),
                "completedSteps": [],
            });
        }
    };

    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<InvokeRequest>();
    let tabs_snapshot = Arc::new(Mutex::new(backend.tabs()));
    let tabs_for_js = Arc::clone(&tabs_snapshot);
    let script_owned = script.to_owned();

    let js_fut = async move {
        let tx = req_tx;
        let result: Result<String, String> = context
            .async_with(async |ctx| {
                let globals = ctx.globals();

                let tx_invoke = tx.clone();
                let invoke_fn = Function::new(
                    ctx.clone(),
                    Async(move |method: String, params_json: String| {
                        let tx = tx_invoke.clone();
                        async move {
                            let answer = async {
                                let params: Value = serde_json::from_str(&params_json)
                                    .map_err(|e| format!("invalid parameters: {e}"))?;
                                let (reply_tx, reply_rx) = oneshot::channel();
                                tx.send(InvokeRequest {
                                    method,
                                    params,
                                    reply: reply_tx,
                                })
                                .map_err(|_| "browser_drive host stopped".to_owned())?;
                                reply_rx
                                    .await
                                    .map_err(|_| "browser_drive call aborted".to_owned())?
                            }
                            .await;
                            let envelope = match answer {
                                Ok(value) => json!({ "ok": value }),
                                Err(error) => json!({ "error": error }),
                            };
                            serde_json::to_string(&envelope)
                                .unwrap_or_else(|_| r#"{"error":"could not encode result"}"#.into())
                        }
                    }),
                )
                .map_err(|e| format!("could not register invoke: {e}"))?;
                globals
                    .set("__sikemuxInvoke", invoke_fn)
                    .map_err(|e| format!("could not set invoke: {e}"))?;

                let tabs_handle = Arc::clone(&tabs_for_js);
                let tabs_fn = Function::new(ctx.clone(), move || -> String {
                    let value = tabs_handle.locked().clone();
                    serde_json::to_string(&value).unwrap_or_else(|_| "[]".to_owned())
                })
                .map_err(|e| format!("could not register tabs: {e}"))?;
                globals
                    .set("__sikemuxTabs", tabs_fn)
                    .map_err(|e| format!("could not set tabs: {e}"))?;

                let sleep_fn = Function::new(
                    ctx.clone(),
                    Async(move |ms: f64| async move {
                        let dur = Duration::from_millis((ms.clamp(0.0, 60_000.0)) as u64);
                        let left = deadline.saturating_duration_since(Instant::now());
                        tokio::time::sleep(dur.min(left)).await;
                    }),
                )
                .map_err(|e| format!("could not register sleep: {e}"))?;
                globals
                    .set("__sikemuxSleep", sleep_fn)
                    .map_err(|e| format!("could not set sleep: {e}"))?;

                ctx.eval::<(), _>(DRIVE_SDK_JS)
                    .catch(&ctx)
                    .map_err(format_caught_error)?;

                let wrapped = format!(
                    "(async () => {{\n\
                     const __val = await (async () => {{\n{script_owned}\n}})();\n\
                     const __seen = new WeakSet();\n\
                     return JSON.stringify(__val === undefined ? null : __val, (_k, v) => {{\n\
                         if (typeof v === 'bigint' || typeof v === 'function' || typeof v === 'symbol') return String(v);\n\
                         if (typeof v === 'object' && v !== null) {{\n\
                             if (__seen.has(v)) return '[circular]';\n\
                             __seen.add(v);\n\
                         }}\n\
                         return v;\n\
                     }}) ?? 'null';\n\
                     }})()"
                );

                let promise: Promise<'_> = ctx
                    .eval(wrapped)
                    .catch(&ctx)
                    .map_err(format_caught_error)?;
                promise
                    .into_future::<String>()
                    .await
                    .catch(&ctx)
                    .map_err(format_caught_error)
            })
            .await;
        drop(tx);
        result
    };
    tokio::pin!(js_fut);

    let sleep_deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(sleep_deadline);

    let mut in_flight: Vec<InFlight<'_>> = Vec::new();
    let mut completed_steps: Vec<Value> = Vec::new();
    let mut next_step: usize = 0;
    let mut held_write: Option<HeldWrite> = None;

    let js_outcome: Option<Result<String, String>> = std::future::poll_fn(|cx| {
        if sleep_deadline.as_mut().poll(cx).is_ready() {
            timed_out.store(true, Ordering::Release);
            return Poll::Ready(None);
        }

        // First poll JS so it can enqueue new primitive calls or finish.
        if let Poll::Ready(result) = js_fut.as_mut().poll(cx) {
            return Poll::Ready(Some(result));
        }

        // Refresh tab snapshot and drain any newly queued primitive calls.
        while let Poll::Ready(Some(req)) = req_rx.poll_recv(cx) {
            if !ALLOWED_DRIVE_METHODS.contains(&req.method.as_str()) {
                let _ = req.reply.send(Err(format!(
                    "{} is not available inside browser_drive",
                    req.method
                )));
                continue;
            }
            if next_step >= MAX_DRIVE_STEPS {
                let _ = req.reply.send(Err(format!(
                    "browser_drive exceeded the {MAX_DRIVE_STEPS}-step limit"
                )));
                continue;
            }
            let step = next_step;
            next_step += 1;
            let summary = step_summary(&req.method, &req.params);
            let tab_id = requested_tab(&req.params).map(str::to_owned);
            let future = backend.call(req.method.clone(), req.params);
            in_flight.push(InFlight {
                step,
                method: req.method,
                tab_id,
                summary,
                reply: Some(req.reply),
                future,
            });
        }

        // Poll all in-flight backend calls concurrently (enables Promise.all across tabs).
        let mut idx = 0;
        let mut any_progressed = false;
        while idx < in_flight.len() {
            let ready = {
                let Some(entry) = in_flight.get_mut(idx) else {
                    break;
                };
                match entry.future.as_mut().poll(cx) {
                    Poll::Ready(res) => Some(res),
                    Poll::Pending => None,
                }
            };
            if let Some(res) = ready {
                let mut entry = in_flight.swap_remove(idx);
                any_progressed = true;
                *tabs_snapshot.locked() = backend.tabs();
                match res {
                    Ok(value) => {
                        let resolved_tab = value
                            .get("tabId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .or(entry.tab_id);
                        completed_steps.push(json!({
                            "step": entry.step,
                            "method": entry.method,
                            "tabId": resolved_tab,
                            "summary": entry.summary,
                        }));
                        if let Some(reply) = entry.reply.take() {
                            let _ = reply.send(Ok(value));
                        }
                    }
                    Err(error) if is_held_write_error(&error) => {
                        gate_aborted.store(true, Ordering::Release);
                        held_write = Some(HeldWrite {
                            step: entry.step,
                            method: entry.method,
                            tab_id: entry.tab_id,
                            summary: entry.summary,
                            error,
                        });
                        in_flight.clear();
                        return Poll::Ready(None);
                    }
                    Err(error) => {
                        if let Some(reply) = entry.reply.take() {
                            let _ = reply.send(Err(error));
                        }
                    }
                }
            } else {
                idx += 1;
            }
        }

        if any_progressed {
            // Re-poll JS immediately so resolved promises continue without waiting another tick.
            if let Poll::Ready(result) = js_fut.as_mut().poll(cx) {
                return Poll::Ready(Some(result));
            }
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    })
    .await;

    // Sort completedSteps by invocation order so parallel Promise.all steps read cleanly.
    completed_steps.sort_by_key(|entry| entry.get("step").and_then(Value::as_u64).unwrap_or(0));

    if let Some(held) = held_write {
        return json!({
            "ok": false,
            "heldAt": {
                "step": held.step,
                "method": held.method,
                "tabId": held.tab_id,
                "summary": held.summary,
            },
            "completedSteps": completed_steps,
            "error": held.error,
        });
    }

    if timed_out.load(Ordering::Acquire) {
        return json!({
            "ok": false,
            "error": format!("browser_drive script timed out after {} ms", timeout.as_millis()),
            "completedSteps": completed_steps,
        });
    }

    match js_outcome {
        Some(Ok(raw)) => {
            if raw.len() > MAX_SCRIPT_RESULT {
                let cut = (0..=MAX_SCRIPT_RESULT)
                    .rev()
                    .find(|at| raw.is_char_boundary(*at))
                    .unwrap_or(0);
                return json!({
                    "ok": true,
                    "result": raw.get(..cut),
                    "truncated": true,
                    "completedSteps": completed_steps,
                });
            }
            let result = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
            json!({
                "ok": true,
                "result": result,
                "completedSteps": completed_steps,
            })
        }
        Some(Err(error)) => json!({
            "ok": false,
            "error": error,
            "completedSteps": completed_steps,
        }),
        None => json!({
            "ok": false,
            "error": format!("browser_drive script timed out after {} ms", timeout.as_millis()),
            "completedSteps": completed_steps,
        }),
    }
}

fn format_caught_error(error: CaughtError<'_>) -> String {
    match error {
        CaughtError::Exception(exc) => {
            let msg = exc.message().unwrap_or_default();
            if msg.is_empty() {
                format!("{exc:?}")
            } else {
                msg
            }
        }
        CaughtError::Value(val) => format!("{val:?}"),
        CaughtError::Error(err) => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct MockBackend {
        tabs: Mutex<Vec<Value>>,
        in_flight_now: AtomicUsize,
        max_concurrent: AtomicUsize,
        call_delay: Duration,
        hold_on_text: Option<String>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                tabs: Mutex::new(vec![
                    json!({ "id": "tab-1", "title": "One", "url": "https://one.test", "active": true, "yours": false }),
                    json!({ "id": "tab-2", "title": "Two", "url": "https://two.test", "active": false, "yours": false }),
                ]),
                in_flight_now: AtomicUsize::new(0),
                max_concurrent: AtomicUsize::new(0),
                call_delay: Duration::from_millis(40),
                hold_on_text: None,
            }
        }
    }

    impl DriveBackend for MockBackend {
        fn call<'a>(
            &'a self,
            method: String,
            params: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + 'a>> {
            Box::pin(async move {
                let current = self.in_flight_now.fetch_add(1, Ordering::AcqRel) + 1;
                self.max_concurrent.fetch_max(current, Ordering::AcqRel);
                if !self.call_delay.is_zero() {
                    tokio::time::sleep(self.call_delay).await;
                }
                self.in_flight_now.fetch_sub(1, Ordering::AcqRel);

                if let Some(needle) = &self.hold_on_text {
                    if params.get("text").and_then(Value::as_str) == Some(needle.as_str()) {
                        return Err(
                            "Waits for the person's yes: `POST https://prod.test/deploy` changes prod.test, a production place."
                                .to_owned(),
                        );
                    }
                }
                match method.as_str() {
                    "browser.navigate" => {
                        let url = params
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or("https://new.test");
                        let tab_id = if params.get("newTab").and_then(Value::as_bool) == Some(true)
                        {
                            let mut tabs = self.tabs.locked();
                            let id = format!("tab-{}", tabs.len() + 1);
                            tabs.push(json!({ "id": id, "title": url, "url": url, "active": false, "yours": false }));
                            id
                        } else {
                            params
                                .get("tabId")
                                .and_then(Value::as_str)
                                .unwrap_or("tab-1")
                                .to_owned()
                        };
                        Ok(json!({ "tabId": tab_id, "url": url, "title": url, "loading": false }))
                    }
                    "browser.state" => {
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(
                            json!({ "tabId": tab_id, "text": format!("body of {tab_id}"), "elements": "" }),
                        )
                    }
                    "browser.click" => {
                        if params.get("text").and_then(Value::as_str) == Some("Missing") {
                            return Err("nothing shows or is labelled \"Missing\"".into());
                        }
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(json!({ "tabId": tab_id, "action": "clicked" }))
                    }
                    "browser.type" => {
                        let tab_id = params
                            .get("tabId")
                            .and_then(Value::as_str)
                            .unwrap_or("tab-1");
                        Ok(json!({
                            "tabId": tab_id,
                            "typed": 5,
                            "selector": params.get("selector").cloned().unwrap_or(Value::Null),
                            "field": params.get("field").cloned().unwrap_or(Value::Null),
                            "text": params.get("text").cloned().unwrap_or(Value::Null),
                            "replace": params.get("replace").cloned().unwrap_or(Value::Null),
                        }))
                    }
                    "browser.select" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "selected": params.get("value").cloned().unwrap_or(Value::Null),
                        "field": params.get("field").cloned().unwrap_or(Value::Null),
                    })),
                    "browser.upload" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "uploaded": params.get("paths").cloned().unwrap_or_else(|| json!([])),
                        "selector": params.get("selector").cloned().unwrap_or(Value::Null),
                    })),
                    "browser.dialog" => {
                        if self.hold_on_text.as_deref() == Some("dialog") {
                            return Err(
                                "Waits for the person's yes: `POST https://prod.test/confirm` changes prod.test, a production place."
                                    .to_owned(),
                            );
                        }
                        Ok(json!({
                            "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                            "accepted": params.get("accept").cloned().unwrap_or(Value::Null),
                            "text": params.get("text").cloned().unwrap_or(Value::Null),
                        }))
                    }
                    "browser.download" => Ok(json!({
                        "tabId": params.get("tabId").and_then(Value::as_str).unwrap_or("tab-1"),
                        "path": "/tmp/desk-downloads/photo.jpg",
                        "mimeType": "image/jpeg",
                        "bytes": 4200,
                        "url": params.get("url").cloned().unwrap_or(Value::Null),
                        "filename": params.get("filename").cloned().unwrap_or(Value::Null),
                    })),
                    _ => Ok(json!({ "ok": true })),
                }
            })
        }

        fn tabs(&self) -> Value {
            Value::Array(self.tabs.locked().clone())
        }
    }

    #[tokio::test]
    async fn browser_drive_runs_promise_all_across_multiple_tabs_in_parallel() {
        let backend = MockBackend::new();
        let script = r#"
            const [t1, t2, t3] = await Promise.all([
                browser.open("https://a.test", { background: true }),
                browser.open("https://b.test", { background: true }),
                browser.open("https://c.test", { background: true }),
            ]);
            const states = await Promise.all([t1.state(), t2.state(), t3.state()]);
            return states.map(s => ({ id: s.tabId, text: s.text }));
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert!(
            backend.max_concurrent.load(Ordering::Acquire) >= 2,
            "expected concurrent execution across tabs, max_concurrent was {}",
            backend.max_concurrent.load(Ordering::Acquire)
        );
        let arr = out["result"].as_array().expect("result array");
        assert_eq!(arr.len(), 3);
        assert_eq!(out["completedSteps"].as_array().map(Vec::len), Some(6));
    }

    #[tokio::test]
    async fn browser_drive_sandbox_has_no_host_globals() {
        let backend = MockBackend::new();
        let script = r#"
            return {
                hasRequire: typeof require !== "undefined",
                hasProcess: typeof process !== "undefined",
                hasFetch: typeof fetch !== "undefined",
                hasWindow: typeof window !== "undefined",
                hasDocument: typeof document !== "undefined",
                hasBrowser: typeof browser === "object" && typeof browser.open === "function",
                tabsCount: browser.tabs().length,
            };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(
            out["result"],
            json!({
                "hasRequire": false,
                "hasProcess": false,
                "hasFetch": false,
                "hasWindow": false,
                "hasDocument": false,
                "hasBrowser": true,
                "tabsCount": 2,
            })
        );
    }

    #[tokio::test]
    async fn browser_drive_enforces_timeout_on_infinite_loop() {
        let backend = MockBackend::new();
        let started = Instant::now();
        let out = run_drive(&backend, "while (true) {}", Duration::from_millis(120)).await;
        assert_eq!(out["ok"], json!(false));
        let err = out["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("timed out"),
            "expected timeout error, got: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout took too long: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn browser_drive_fails_fast_on_held_write_gate_with_step_ledger() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        backend.hold_on_text = Some("Deploy".into());
        let script = r#"
            const tab = browser.tab("tab-1");
            await tab.type("Email", "a@b.test");
            // Even wrapped in try/catch or optional, a held write gate must fail fast and never be swallowed.
            try {
                await tab.click("Deploy", { optional: true });
            } catch {}
            await tab.click("AfterDeploy");
            return "should never reach";
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected fail-fast: {out}");
        assert_eq!(out["heldAt"]["method"], json!("browser.click"));
        assert_eq!(out["heldAt"]["tabId"], json!("tab-1"));
        assert_eq!(out["heldAt"]["step"], json!(1));
        assert!(out["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Waits for the person"));
        let completed = out["completedSteps"].as_array().expect("completedSteps");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["method"], json!("browser.type"));
    }

    #[tokio::test]
    async fn browser_drive_optional_swallows_normal_errors() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r#"
            const tab = browser.tab("tab-1");
            const skipped = await tab.click("Missing", { optional: true });
            const st = await tab.state();
            return { skipped, text: st.text };
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["skipped"]["skipped"], json!(true));
        assert_eq!(out["result"]["text"], json!("body of tab-1"));
    }

    #[tokio::test]
    async fn browser_drive_enforces_memory_limit() {
        let backend = MockBackend::new();
        let script = r#"
            const chunks = [];
            for (let i = 0; i < 200; i++) {
                chunks.push("x".repeat(1024 * 1024));
            }
            return chunks.length;
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected OOM failure: {out}");
        assert!(
            out["error"].as_str().is_some_and(|e| !e.is_empty()),
            "expected non-empty error: {out}"
        );
    }

    #[tokio::test]
    async fn browser_drive_tab_exposes_select_upload_dialog_and_download() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const sel = await tab.select("#state", "Karnataka");
            const up = await tab.upload('input[type="file"]', ["/tmp/pan.pdf"]);
            const dlg = await tab.dialog("accept", "confirmed");
            const dl = await tab.download("https://lh3.googleusercontent.com/abc", { filename: "photo.jpg" });
            return { sel, up, dlg, dl };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["sel"]["selected"], json!("Karnataka"));
        assert_eq!(out["result"]["sel"]["field"], json!("#state"));
        assert_eq!(out["result"]["up"]["uploaded"], json!(["/tmp/pan.pdf"]));
        assert_eq!(
            out["result"]["up"]["selector"],
            json!("input[type=\"file\"]")
        );
        assert_eq!(out["result"]["dlg"]["accepted"], json!(true));
        assert_eq!(out["result"]["dlg"]["text"], json!("confirmed"));
        assert_eq!(
            out["result"]["dl"]["path"],
            json!("/tmp/desk-downloads/photo.jpg")
        );
        assert_eq!(out["result"]["dl"]["mimeType"], json!("image/jpeg"));
        assert_eq!(out["result"]["dl"]["bytes"], json!(4200));
        assert_eq!(out["result"]["dl"]["filename"], json!("photo.jpg"));
    }

    #[tokio::test]
    async fn browser_drive_dialog_respects_fail_fast_write_gate() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        backend.hold_on_text = Some("dialog".into());
        let script = r#"
            const tab = browser.tab("tab-1");
            try {
                await tab.dialog("accept", { optional: true });
            } catch {}
            return "unreachable";
        "#;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(false), "expected held gate: {out}");
        assert_eq!(out["heldAt"]["method"], json!("browser.dialog"));
        assert!(out["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Waits for the person"));
    }

    #[tokio::test]
    async fn browser_drive_type_merges_target_object_with_text_and_opts() {
        let mut backend = MockBackend::new();
        backend.call_delay = Duration::ZERO;
        let script = r##"
            const tab = browser.tab("tab-1");
            const a = await tab.type({ selector: "#q" }, "hello");
            const b = await tab.type({ selector: "#q" }, "world", { replace: true });
            const c = await tab.type({ selector: "#q", text: "inline" });
            return { a, b, c };
        "##;
        let out = run_drive(&backend, script, Duration::from_secs(5)).await;
        assert_eq!(out["ok"], json!(true), "unexpected output: {out}");
        assert_eq!(out["result"]["a"]["selector"], json!("#q"));
        assert_eq!(out["result"]["a"]["text"], json!("hello"));
        assert_eq!(out["result"]["b"]["selector"], json!("#q"));
        assert_eq!(out["result"]["b"]["text"], json!("world"));
        assert_eq!(out["result"]["b"]["replace"], json!(true));
        assert_eq!(out["result"]["c"]["selector"], json!("#q"));
        assert_eq!(out["result"]["c"]["text"], json!("inline"));
    }
}
