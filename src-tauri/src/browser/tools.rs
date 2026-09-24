//! The agent's browser tools, answered over the harness socket. Every method
//! acts on the agent's own tabs, the same ones the person sees in the pane.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview};

use super::{BrowserManager, BLANK_URL};
use crate::harness::HarnessRequest;

const PAGE_SCRIPT: &str = include_str!("page.js");
const EVAL_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_millis(250);
const MAX_WAIT_MS: u64 = 30_000;
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SCRIPT_RESULT: usize = 100_000;
/// A full-page capture goes through a PDF page, and PDF stops at 14,400 points.
const MAX_PAGE_HEIGHT: f64 = 14_400.0;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_UPLOAD_FILES: usize = 20;
const DRAG_STEPS: u32 = 12;
const DRAG_STEP_DELAY: Duration = Duration::from_millis(16);

use crate::generated_agent_tools::BROWSER_METHODS as METHODS;
use native::Mouse;

pub fn is_browser_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Runs on a CLI broker thread, so blocking on the async runtime is safe.
pub fn execute(app: &AppHandle, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("browser tools need the agent's id")?;
    let manager = app.state::<BrowserManager>();
    let acts_on_a_tab = !matches!(
        request.method.as_str(),
        "browser.tabs" | "browser.tab.close"
    );
    let mut marks = Vec::new();
    if acts_on_a_tab {
        manager.announce_acting(app, agent_id);
        marks.extend(manager.mark_acting(app, agent_id));
    }
    let result =
        tauri::async_runtime::block_on(run(app, agent_id, &request.method, &request.params))
            .map_err(|error| error.to_string());
    if acts_on_a_tab {
        marks.extend(manager.mark_acting(app, agent_id));
        manager.release_acting(app, agent_id, marks);
    }
    result
}

async fn run(
    app: &AppHandle,
    agent_id: &str,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
    let manager = app.state::<BrowserManager>();
    let text = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_owned);
    let index = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_u64)
            .map(|value| value as usize)
    };
    match method {
        "browser.tabs" => Ok(tabs(&manager, agent_id)),
        "browser.tab.switch" => {
            let id = text("tabId").ok_or("tabId is required")?;
            manager
                .switch_tab(app, agent_id, &id)
                .map_err(|error| error.to_string())?;
            state(&manager, agent_id).await
        }
        "browser.tab.close" => {
            let id = text("tabId").ok_or("tabId is required")?;
            manager
                .close_tab(app, agent_id, &id)
                .map_err(|error| error.to_string())?;
            Ok(tabs(&manager, agent_id))
        }
        "browser.navigate" => {
            let url = text("url").ok_or("url is required")?;
            let tab_id = if params.get("newTab").and_then(Value::as_bool) == Some(true)
                || manager.active_view(agent_id).is_err()
            {
                manager
                    .open_tab(app, agent_id, Some(&url))
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                manager
                    .navigate(app, agent_id, &url)
                    .map_err(|error| error.to_string())?;
                manager
                    .active_view(agent_id)
                    .map_err(|error| error.to_string())?
                    .0
            };
            let _ = manager
                .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                .await;
            state(&manager, agent_id).await
        }
        "browser.state" => state(&manager, agent_id).await,
        "browser.click" => {
            let (tab_id, view) = active(&manager, agent_id)?;
            let (x, y, mut result) = target(&view, params, "index", "x", "y").await?;
            let hover = params.get("hover").and_then(Value::as_bool) == Some(true);
            let clicks = if params.get("double").and_then(Value::as_bool) == Some(true) {
                2
            } else {
                1
            };
            show_pointer(&view, x, y, !hover).await;
            native::mouse(&view, Mouse::Move, x, y, 0).await?;
            if hover {
                result["hover"] = call(&view, "hover", &[json!(x), json!(y)]).await?;
            }
            if !hover {
                for count in 1..=clicks {
                    native::mouse(&view, Mouse::Down, x, y, count).await?;
                    native::mouse(&view, Mouse::Up, x, y, count).await?;
                }
            }
            result["action"] = json!(if hover {
                "hovered"
            } else if clicks == 2 {
                "double-clicked"
            } else {
                "clicked"
            });
            settle(&manager, agent_id, &tab_id).await;
            merge(result, state(&manager, agent_id).await?)
        }
        "browser.upload" => {
            let paths = params
                .get("paths")
                .and_then(Value::as_array)
                .ok_or("paths is required")?
                .iter()
                .map(|path| {
                    let path =
                        std::path::PathBuf::from(path.as_str().ok_or("paths must be strings")?);
                    if !path.is_absolute() {
                        return Err(format!("{} is not an absolute path", path.display()));
                    }
                    if !path.is_file() {
                        return Err(format!("{} is not a file", path.display()));
                    }
                    Ok(path)
                })
                .collect::<Result<Vec<_>, String>>()?;
            if paths.is_empty() || paths.len() > MAX_UPLOAD_FILES {
                return Err(format!("pass between 1 and {MAX_UPLOAD_FILES} files"));
            }
            let (tab_id, view) = active(&manager, agent_id)?;
            let (x, y, mut result) = target(&view, params, "index", "x", "y").await?;
            let names: Vec<String> = paths
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            manager.offer_upload(&tab_id, paths);
            show_pointer(&view, x, y, true).await;
            native::mouse(&view, Mouse::Down, x, y, 1).await?;
            native::mouse(&view, Mouse::Up, x, y, 1).await?;
            let started = Instant::now();
            while manager.upload_pending(&tab_id) {
                if started.elapsed() >= UPLOAD_TIMEOUT {
                    manager.take_upload(&tab_id);
                    return Err("that did not open a file chooser; pass the number of a file input, or of the button that opens one".into());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            result["uploaded"] = json!(names);
            settle(&manager, agent_id, &tab_id).await;
            merge(result, state(&manager, agent_id).await?)
        }
        "browser.drag" => {
            let (tab_id, view) = active(&manager, agent_id)?;
            let (from_x, from_y, from) =
                target(&view, params, "fromIndex", "fromX", "fromY").await?;
            let (to_x, to_y, to) = target(&view, params, "toIndex", "toX", "toY").await?;
            let dragged = call(
                &view,
                "html5Drag",
                &[json!(from_x), json!(from_y), json!(to_x), json!(to_y)],
            )
            .await?;
            show_pointer(&view, from_x, from_y, false).await;
            if dragged.get("dropped").is_none() {
                native::mouse(&view, Mouse::Move, from_x, from_y, 0).await?;
                native::mouse(&view, Mouse::Down, from_x, from_y, 1).await?;
                for step in 1..=DRAG_STEPS {
                    let progress = f64::from(step) / f64::from(DRAG_STEPS);
                    tokio::time::sleep(DRAG_STEP_DELAY).await;
                    native::mouse(
                        &view,
                        Mouse::Drag,
                        from_x + (to_x - from_x) * progress,
                        from_y + (to_y - from_y) * progress,
                        1,
                    )
                    .await?;
                }
                native::mouse(&view, Mouse::Up, to_x, to_y, 1).await?;
            }
            show_pointer(&view, to_x, to_y, false).await;
            settle(&manager, agent_id, &tab_id).await;
            merge(
                json!({ "from": from, "to": to, "dragged": dragged }),
                state(&manager, agent_id).await?,
            )
        }
        "browser.type" => {
            let value = text("text").ok_or("text is required")?;
            let submit = params
                .get("submit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (tab_id, view) = active(&manager, agent_id)?;
            let at = index("index")
                .map(|value| json!(value))
                .unwrap_or(Value::Null);
            let prepared = call(&view, "focus", &[at.clone(), json!(value)]).await?;
            if prepared.get("selected").is_some() {
                return Ok(prepared);
            }
            let replacing = prepared.get("replacing").and_then(Value::as_bool) == Some(true);
            if let Some(point) = prepared.get("clickFirst") {
                let coordinate = |key: &str| point.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                let (x, y) = (coordinate("x"), coordinate("y"));
                native::mouse(&view, Mouse::Down, x, y, 1).await?;
                native::mouse(&view, Mouse::Up, x, y, 1).await?;
            }
            if !value.is_empty() {
                native::insert_text(&view, &value).await?;
            } else if replacing {
                native::key(&view, "Backspace").await?;
            }
            let typed = merge(
                json!({ "typed": value.chars().count(), "replaced": replacing, "submitted": submit }),
                call(&view, "valueOf", &[at]).await?,
            )?;
            if submit {
                native::key(&view, "Enter").await?;
                settle(&manager, agent_id, &tab_id).await;
                return merge(typed, state(&manager, agent_id).await?);
            }
            Ok(typed)
        }
        "browser.press" => {
            let key = text("key").ok_or("key is required")?;
            let (tab_id, view) = active(&manager, agent_id)?;
            native::key(&view, &key).await?;
            settle(&manager, agent_id, &tab_id).await;
            merge(json!({ "pressed": key }), state(&manager, agent_id).await?)
        }
        "browser.dialog" => {
            let accept = params
                .get("accept")
                .and_then(Value::as_bool)
                .ok_or("accept is required")?;
            let (tab_id, view) = active_tab(&manager, agent_id)?;
            if manager.dialog(&tab_id).is_none() {
                return Err("no dialog is open in the current tab".into());
            }
            native::answer_dialog(&view, &tab_id, accept, text("text")).await?;
            settle(&manager, agent_id, &tab_id).await;
            state(&manager, agent_id).await
        }
        "browser.scroll" => {
            let delta = params
                .get("deltaY")
                .and_then(Value::as_f64)
                .unwrap_or(600.0)
                .clamp(-20_000.0, 20_000.0);
            let (_, view) = active(&manager, agent_id)?;
            call(
                &view,
                "scroll",
                &[
                    json!(delta),
                    index("index")
                        .map(|value| json!(value))
                        .unwrap_or(Value::Null),
                ],
            )
            .await
        }
        "browser.network" => {
            let (_, view) = active(&manager, agent_id)?;
            call(
                &view,
                "network",
                &[
                    params
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(|value| json!(value))
                        .unwrap_or(Value::Null),
                    text("filter")
                        .map(|value| json!(value))
                        .unwrap_or(Value::Null),
                ],
            )
            .await
        }
        "browser.evaluate" => {
            let script = text("script").ok_or("script is required")?;
            let (_, view) = active(&manager, agent_id)?;
            let answer =
                match native::run_script(&view, &script_body(&format!("return ({script}\n);")))
                    .await
                {
                    Err(error) if error.contains("SyntaxError") => {
                        native::run_script(&view, &script_body(&script)).await
                    }
                    other => other,
                }?;
            if answer.len() > MAX_SCRIPT_RESULT {
                let cut = (0..=MAX_SCRIPT_RESULT)
                    .rev()
                    .find(|at| answer.is_char_boundary(*at))
                    .unwrap_or(0);
                return Ok(json!({ "result": answer.get(..cut), "truncated": true }));
            }
            Ok(json!({
                "result": serde_json::from_str::<Value>(&answer).unwrap_or(Value::String(answer)),
            }))
        }
        "browser.console" => {
            let (_, view) = active(&manager, agent_id)?;
            call(
                &view,
                "console",
                &[
                    params
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(|value| json!(value))
                        .unwrap_or(Value::Null),
                    json!(params.get("errors").and_then(Value::as_bool) == Some(true)),
                ],
            )
            .await
        }
        "browser.extract" => {
            let (_, view) = active(&manager, agent_id)?;
            call(
                &view,
                "extract",
                &[text("selector")
                    .map(|value| json!(value))
                    .unwrap_or(Value::Null)],
            )
            .await
        }
        "browser.screenshot" => {
            let (tab_id, view) = active(&manager, agent_id)?;
            let annotate = params.get("annotate").and_then(Value::as_bool) == Some(true);
            let full_page = params.get("fullPage").and_then(Value::as_bool) == Some(true);
            let marked = if annotate {
                let page = state(&manager, agent_id).await?;
                call(&view, "showMarks", &[json!(true)]).await?;
                page.get("elements").cloned()
            } else {
                let _ = call(&view, "pointerVisible", &[json!(false)]).await;
                None
            };
            let height = if full_page {
                call(&view, "pageHeight", &[])
                    .await?
                    .as_f64()
                    .map(Some)
                    .ok_or("the page did not report its height")?
            } else {
                None
            };
            let image = screenshot(&view, height).await;
            let _ = if annotate {
                call(&view, "showMarks", &[json!(false)]).await
            } else {
                call(&view, "pointerVisible", &[json!(true)]).await
            };
            let page = manager.page(agent_id, &tab_id).unwrap_or_default();
            let mut result = json!({
                "tabId": tab_id,
                "url": page.url,
                "title": page.title,
                "mimeType": "image/jpeg",
                "data": base64_encode(&image?),
            });
            if let Some(elements) = marked {
                result["elements"] = elements;
            }
            if height.is_some_and(|height| height > MAX_PAGE_HEIGHT) {
                result["cutAt"] = json!(MAX_PAGE_HEIGHT);
            }
            Ok(result)
        }
        "browser.record" => match text("action").as_deref() {
            Some("start") => native::start_recording(app, agent_id, text("path")).await,
            Some("stop") => native::stop_recording(app, agent_id).await,
            _ => Err("action must be start or stop".into()),
        },
        "browser.annotate" => {
            let (_, view) = active(&manager, agent_id)?;
            let clear = params.get("clear").and_then(Value::as_bool) == Some(true);
            let placing = ["index", "x", "text"]
                .iter()
                .any(|key| params.get(*key).is_some());
            let cleared = if clear {
                call(&view, "clearAnnotations", &[]).await?
            } else {
                json!({})
            };
            if !placing {
                return Ok(cleared);
            }
            let number = |key: &str| params.get(key).cloned().unwrap_or(Value::Null);
            merge(
                call(
                    &view,
                    "annotate",
                    &[
                        number("index"),
                        number("x"),
                        number("y"),
                        number("text"),
                        number("durationMs"),
                    ],
                )
                .await?,
                cleared,
            )
        }
        "browser.wait" => {
            let ms = params
                .get("ms")
                .and_then(Value::as_u64)
                .unwrap_or(1000)
                .min(MAX_WAIT_MS);
            let (tab_id, _) = active(&manager, agent_id)?;
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let _ = manager
                .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                .await;
            state(&manager, agent_id).await
        }
        "browser.back" | "browser.forward" => {
            let (tab_id, _) = active(&manager, agent_id)?;
            manager
                .history(agent_id, if method == "browser.back" { -1 } else { 1 })
                .map_err(|error| error.to_string())?;
            settle(&manager, agent_id, &tab_id).await;
            state(&manager, agent_id).await
        }
        _ => Err("unknown browser method".into()),
    }
}

/// Wraps an agent's script so whatever it returns, awaited, comes back as JSON.
/// Elements become their markup, since JSON has no way to spell a node.
fn script_body(script: &str) -> String {
    format!(
        r#"const value = await (async () => {{
{script}
}})();
const seen = new WeakSet();
try {{
    return JSON.stringify(value === undefined ? null : value, (key, item) => {{
    if (typeof Node === "function" && item instanceof Node) return String(item.outerHTML ?? item.textContent ?? "").slice(0, 2000);
    if (typeof item === "bigint" || typeof item === "function" || typeof item === "symbol") return String(item);
    if (typeof item === "object" && item !== null) {{
        if (seen.has(item)) return "[circular]";
        seen.add(item);
    }}
    return item;
    }}) ?? "null";
}} catch {{
    return JSON.stringify(String(value));
}}"#
    )
}

/// Draws the agent's pointer where it is about to act, so the person, and any
/// recording, can follow along. A page that refuses the drawing still acts.
async fn show_pointer(view: &Webview, x: f64, y: f64, ripple: bool) {
    let _ = call(view, "mark", &[json!(x), json!(y), json!(ripple)]).await;
}

/// Where to point: the centre of a numbered element, scrolled into view, or
/// CSS pixel coordinates in the tab's viewport.
async fn target(
    view: &Webview,
    params: &Value,
    index_key: &str,
    x_key: &str,
    y_key: &str,
) -> Result<(f64, f64, Value), String> {
    if let Some(index) = params.get(index_key).and_then(Value::as_u64) {
        let point = call(view, "point", &[json!(index)]).await?;
        let coordinate = |key: &str| {
            point
                .get(key)
                .and_then(Value::as_f64)
                .ok_or_else(|| "the page returned no position".to_string())
        };
        return Ok((coordinate("x")?, coordinate("y")?, point));
    }
    match (
        params.get(x_key).and_then(Value::as_f64),
        params.get(y_key).and_then(Value::as_f64),
    ) {
        (Some(x), Some(y)) => Ok((x, y, json!({ "x": x, "y": y }))),
        _ => Err(format!("pass {index_key}, or both {x_key} and {y_key}")),
    }
}

/// Input that reaches the page the way a person's does, as trusted events.
mod native {
    use tauri::Webview;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mouse {
        Move,
        Down,
        Drag,
        Up,
    }

    #[cfg(target_os = "macos")]
    mod platform {
        use super::super::super::input;
        use super::super::on_tab;
        use super::Mouse;
        use tauri::Webview;

        pub async fn mouse(
            view: &Webview,
            kind: Mouse,
            x: f64,
            y: f64,
            clicks: isize,
        ) -> Result<(), String> {
            let kind = match kind {
                Mouse::Move => input::Mouse::Move,
                Mouse::Down => input::Mouse::Down,
                Mouse::Drag => input::Mouse::Drag,
                Mouse::Up => input::Mouse::Up,
            };
            on_tab(view, move |tab| input::mouse(tab, kind, x, y, clicks)).await
        }

        pub async fn key(view: &Webview, name: &str) -> Result<(), String> {
            let stroke = input::parse_key(name)?;
            on_tab(view, move |tab| input::key(tab, &stroke)).await
        }

        pub async fn insert_text(view: &Webview, text: &str) -> Result<(), String> {
            let text = text.to_owned();
            on_tab(view, move |tab| input::insert_text(tab, &text)).await
        }

        pub async fn start_recording(
            app: &tauri::AppHandle,
            agent_id: &str,
            path: Option<String>,
        ) -> Result<serde_json::Value, String> {
            super::super::super::recording::start(app, agent_id, path).await
        }

        pub async fn stop_recording(
            app: &tauri::AppHandle,
            agent_id: &str,
        ) -> Result<serde_json::Value, String> {
            super::super::super::recording::stop(app, agent_id).await
        }

        pub async fn run_script(view: &Webview, body: &str) -> Result<String, String> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let body = body.to_owned();
            view.with_webview(move |platform| {
                super::super::super::macos::call_async(
                    platform.inner(),
                    &body,
                    Box::new(move |result| {
                        let _ = sender.send(result);
                    }),
                );
            })
            .map_err(|error| error.to_string())?;
            match tokio::time::timeout(super::super::SCRIPT_TIMEOUT, receiver).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("the tab went away".into()),
                Err(_) => Err("the script did not finish within 30 seconds".into()),
            }
        }

        pub async fn answer_dialog(
            view: &Webview,
            tab_id: &str,
            accept: bool,
            text: Option<String>,
        ) -> Result<(), String> {
            let tab_id = tab_id.to_owned();
            on_tab(view, move |_| {
                super::super::super::macos::answer_dialog(&tab_id, accept, text.as_deref())
            })
            .await
        }
    }

    #[cfg(not(target_os = "macos"))]
    mod platform {
        use super::Mouse;
        use tauri::Webview;

        const UNSUPPORTED: &str = "browser input is not available on this platform yet";

        pub async fn mouse(_: &Webview, _: Mouse, _: f64, _: f64, _: isize) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn key(_: &Webview, _: &str) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn insert_text(_: &Webview, _: &str) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn run_script(_: &Webview, _: &str) -> Result<String, String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn start_recording(
            _: &tauri::AppHandle,
            _: &str,
            _: Option<String>,
        ) -> Result<serde_json::Value, String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn stop_recording(
            _: &tauri::AppHandle,
            _: &str,
        ) -> Result<serde_json::Value, String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn answer_dialog(
            _: &Webview,
            _: &str,
            _: bool,
            _: Option<String>,
        ) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }
    }

    pub async fn mouse(
        view: &Webview,
        kind: Mouse,
        x: f64,
        y: f64,
        clicks: isize,
    ) -> Result<(), String> {
        platform::mouse(view, kind, x, y, clicks).await
    }

    pub async fn key(view: &Webview, name: &str) -> Result<(), String> {
        platform::key(view, name).await
    }

    pub async fn insert_text(view: &Webview, text: &str) -> Result<(), String> {
        platform::insert_text(view, text).await
    }

    pub async fn run_script(view: &Webview, body: &str) -> Result<String, String> {
        platform::run_script(view, body).await
    }

    pub async fn start_recording(
        app: &tauri::AppHandle,
        agent_id: &str,
        path: Option<String>,
    ) -> Result<serde_json::Value, String> {
        platform::start_recording(app, agent_id, path).await
    }

    pub async fn stop_recording(
        app: &tauri::AppHandle,
        agent_id: &str,
    ) -> Result<serde_json::Value, String> {
        platform::stop_recording(app, agent_id).await
    }

    pub async fn answer_dialog(
        view: &Webview,
        tab_id: &str,
        accept: bool,
        text: Option<String>,
    ) -> Result<(), String> {
        platform::answer_dialog(view, tab_id, accept, text).await
    }
}

/// Runs `act` against the tab's native view on the main thread.
#[cfg(target_os = "macos")]
async fn on_tab<T: Send + 'static>(
    view: &Webview,
    act: impl FnOnce(*mut std::ffi::c_void) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    view.with_webview(move |platform| {
        let _ = sender.send(act(platform.inner()));
    })
    .map_err(|error| error.to_string())?;
    match tokio::time::timeout(EVAL_TIMEOUT, receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the tab went away".into()),
        Err(_) => Err("the tab took too long to answer".into()),
    }
}

/// The tab the agent is on, unless a dialog has frozen its page: a page
/// waiting on an alert answers nothing, so calling into it would only time out.
fn active(manager: &BrowserManager, agent_id: &str) -> Result<(String, Webview), String> {
    let (tab_id, view) = active_tab(manager, agent_id)?;
    match manager.dialog(&tab_id) {
        Some(dialog) => Err(format!(
            "the page is waiting on a {} dialog saying \"{}\"; answer it with browser_dialog",
            dialog.kind, dialog.message
        )),
        None => Ok((tab_id, view)),
    }
}

fn active_tab(manager: &BrowserManager, agent_id: &str) -> Result<(String, Webview), String> {
    manager
        .active_view(agent_id)
        .map_err(|_| "no browser tab is open; call browser_navigate first".to_string())
}

fn tabs(manager: &BrowserManager, agent_id: &str) -> Value {
    let snapshot = manager
        .snapshot(agent_id)
        .unwrap_or_else(|_| super::BrowserSnapshot {
            tabs: Vec::new(),
            active_tab_id: None,
        });
    json!({
        "activeTabId": snapshot.active_tab_id,
        "tabs": snapshot.tabs.iter().map(|tab| json!({
            "tabId": tab.id,
            "url": tab.url,
            "title": tab.title,
            "active": tab.active,
            "loading": tab.loading,
        })).collect::<Vec<_>>(),
    })
}

async fn state(manager: &BrowserManager, agent_id: &str) -> Result<Value, String> {
    let (tab_id, view) = active_tab(manager, agent_id)?;
    let page = manager.page(agent_id, &tab_id).unwrap_or_default();
    let mut result = if let Some(dialog) = manager.dialog(&tab_id) {
        json!({
            "url": page.url,
            "title": page.title,
            "dialog": dialog,
            "note": "the page is frozen until this dialog is answered with browser_dialog",
        })
    } else if page.url == BLANK_URL {
        json!({ "url": BLANK_URL, "title": "", "elements": "", "text": "" })
    } else {
        call(&view, "state", &[]).await?
    };
    if let Value::Object(map) = &mut result {
        map.insert("tabId".into(), json!(tab_id));
        map.insert("loading".into(), json!(page.loading));
        map.insert("tabs".into(), tabs(manager, agent_id)["tabs"].clone());
    }
    Ok(result)
}

fn merge(mut action: Value, state: Value) -> Result<Value, String> {
    if let (Value::Object(target), Value::Object(source)) = (&mut action, state) {
        for (key, value) in source {
            target.entry(key).or_insert(value);
        }
    }
    Ok(action)
}

/// A click or key can start a navigation that only registers a moment later,
/// so give the page a beat and then wait out any load it started.
async fn settle(manager: &BrowserManager, agent_id: &str, tab_id: &str) {
    tokio::time::sleep(SETTLE).await;
    let _ = manager
        .wait_until_loaded(agent_id, tab_id, LOAD_TIMEOUT)
        .await;
}

/// Call one of the page script's functions and parse what it returns. The
/// script always answers with a JSON string, because WebKit refuses to hand
/// back anything it cannot serialize.
async fn call(view: &Webview, function: &str, args: &[Value]) -> Result<Value, String> {
    let args = args
        .iter()
        .map(|arg| serde_json::to_string(arg).unwrap_or_else(|_| "null".into()))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "{PAGE_SCRIPT}\nJSON.stringify((() => {{ try {{ return {{ ok: window.__sikemux.{function}({args}) }}; }} catch (error) {{ return {{ error: String((error && error.message) || error) }}; }} }})())"
    );
    let raw = eval(view, &script).await?;
    let outer: Value =
        serde_json::from_str(&raw).map_err(|_| "the page returned no answer".to_string())?;
    let inner = match outer {
        Value::String(text) => serde_json::from_str::<Value>(&text)
            .map_err(|_| "the page returned an unreadable answer".to_string())?,
        other => other,
    };
    if let Some(error) = inner.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    inner
        .get("ok")
        .cloned()
        .ok_or_else(|| "the page returned no answer".to_string())
}

pub(super) async fn eval(view: &Webview, script: &str) -> Result<String, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = std::sync::Mutex::new(Some(sender));
    view.eval_with_callback(script, move |result| {
        if let Some(sender) = sender.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = sender.send(result);
        }
    })
    .map_err(|error| error.to_string())?;
    match tokio::time::timeout(EVAL_TIMEOUT, receiver).await {
        Ok(Ok(result)) if !result.is_empty() => Ok(result),
        Ok(_) => Err("the page did not answer; it may still be loading".into()),
        Err(_) => Err("the page took too long to answer".into()),
    }
}

/// The visible part of the tab, or with `height` the whole page down to it.
async fn screenshot(view: &Webview, height: Option<f64>) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let sender = std::sync::Mutex::new(Some(sender));
        view.with_webview(move |platform| {
            let done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> = Box::new(move |result| {
                if let Some(sender) = sender.lock().ok().and_then(|mut slot| slot.take()) {
                    let _ = sender.send(result);
                }
            });
            match height {
                Some(height) => {
                    super::macos::full_page_jpeg(platform.inner(), height, MAX_PAGE_HEIGHT, done)
                }
                None => super::macos::snapshot_jpeg(platform.inner(), done),
            }
        })
        .map_err(|error| error.to_string())?;
        match tokio::time::timeout(EVAL_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("screenshot was abandoned".into()),
            Err(_) => Err("screenshot took too long".into()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (view, height);
        Err("screenshots are not available on this platform yet".into())
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl BrowserManager {
    pub fn page(&self, agent_id: &str, tab_id: &str) -> Option<super::TabPage> {
        self.lock()
            .get(agent_id)
            .and_then(|agent| agent.strip.pages.get(tab_id).cloned())
    }

    /// Resolves once the tab reports its load finished, or at the deadline.
    /// Returns whether it finished.
    pub async fn wait_until_loaded(&self, agent_id: &str, tab_id: &str, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            match self.page(agent_id, tab_id) {
                Some(page) if !page.loading => return true,
                None => return false,
                _ => {}
            }
            if started.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_method_is_namespaced_and_known() {
        assert!(METHODS.iter().all(|method| method.starts_with("browser.")));
        assert!(is_browser_method("browser.click"));
        assert!(!is_browser_method("workspace.inspect"));
        assert!(!is_browser_method("browser.evil"));
    }

    #[test]
    fn page_answers_merge_under_the_action_result() {
        let merged = merge(
            json!({ "clicked": "Sign in", "url": "https://a/next" }),
            json!({ "url": "https://a/next", "title": "Next", "elements": "" }),
        )
        .unwrap();
        assert_eq!(merged["clicked"], "Sign in");
        assert_eq!(merged["title"], "Next");
    }

    /// A dispatched click is untrusted, and pages that check refuse it.
    #[test]
    fn clicks_and_keys_are_never_played_by_the_page_script() {
        for synthetic in [r#""click""#, r#""mousedown""#, r#""keydown""#] {
            assert!(
                !PAGE_SCRIPT.contains(synthetic),
                "page.js dispatches {synthetic}"
            );
        }
    }

    #[test]
    fn the_page_script_is_wrapped_as_a_single_json_string_expression() {
        assert!(PAGE_SCRIPT.contains("window.__sikemux = {"));
        assert!(!PAGE_SCRIPT.contains("`\n"));
    }
}
