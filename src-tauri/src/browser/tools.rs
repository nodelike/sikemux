//! The agent's browser tools, answered over the harness socket. Every method
//! acts on the agent's own tabs, the same ones the person sees in the pane.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview};

use super::viewport::Viewport;
use super::{BrowserManager, BLANK_URL};
use sikemux_core::cli::protocol::HarnessRequest;

const PAGE_SCRIPT: &str = include_str!("page.js");
const RECORDS_SCRIPT: &str = include_str!("records.js");
const EVAL_TIMEOUT: Duration = Duration::from_secs(10);
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_millis(250);
const MAX_WAIT_MS: u64 = 30_000;
const SCRIPT_TIMEOUT_MS: u64 = 30_000;
/// The sidecar stops waiting for any reply after 70 seconds.
const MAX_BLOCKING_MS: u64 = 60_000;
const DEFAULT_CONDITION_WAIT_MS: u64 = 10_000;
const CONDITION_POLL: Duration = Duration::from_millis(200);
const NETWORK_QUIET: Duration = Duration::from_millis(500);
const MAX_SCRIPT_RESULT: usize = 100_000;
/// A full-page capture goes through a PDF page, and PDF stops at 14,400 points.
const MAX_PAGE_HEIGHT: f64 = 14_400.0;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_UPLOAD_FILES: usize = 20;
const DRAG_STEPS: u32 = 12;
const DRAG_STEP_DELAY: Duration = Duration::from_millis(16);
const NARROW_VIEWPORT: u64 = 700;
/// How long a page may take to answer before WebKit is asked whether it hangs.
const SLOW_ANSWER: Duration = Duration::from_secs(1);

use native::Mouse;

/// Runs on a CLI broker thread, so blocking on the async runtime is safe.
pub fn execute(app: &AppHandle, request: &HarnessRequest) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("browser tools need the agent's id")?;
    let manager = app.state::<BrowserManager>();
    let acts_on_a_tab = request.method != "browser.tab.close";
    let mut marks = Vec::new();
    if acts_on_a_tab {
        manager.announce_acting(app, agent_id);
        marks.extend(manager.mark_acting(app, agent_id));
    }
    let before = tab_ids(&manager, agent_id);
    let sends_input = matches!(
        request.method.as_str(),
        "browser.click"
            | "browser.type"
            | "browser.press"
            | "browser.act"
            | "browser.drag"
            | "browser.upload"
    );
    let held = sends_input
        .then(|| manager.active_view(agent_id).ok())
        .flatten()
        .map(|(_, view)| view);
    if let Some(view) = &held {
        let _ = tauri::async_runtime::block_on(native::hold_person_focus(view));
    }
    let mut result =
        tauri::async_runtime::block_on(run(app, agent_id, &request.method, &request.params))
            .map_err(|error| error.to_string());
    if let Some(view) = &held {
        let _ = tauri::async_runtime::block_on(native::return_person_focus(view));
    }
    if acts_on_a_tab {
        marks.extend(manager.mark_acting(app, agent_id));
        manager.release_acting(app, agent_id, marks);
    }
    if request.method != "browser.navigate" {
        if let Ok(Value::Object(map)) = &mut result {
            let opened = opened_tabs(&manager, agent_id, &before);
            if !opened.is_empty() {
                map.insert("openedTabs".into(), json!(opened));
            }
        }
    }
    result
}

fn tab_ids(manager: &BrowserManager, agent_id: &str) -> Vec<String> {
    manager
        .snapshot(agent_id)
        .map(|snapshot| snapshot.tabs.into_iter().map(|tab| tab.id).collect())
        .unwrap_or_default()
}

/// Tabs the page opened during an action, such as a link with a new-window
/// target, which leave the agent on a tab it did not choose.
fn opened_tabs(manager: &BrowserManager, agent_id: &str, before: &[String]) -> Vec<Value> {
    manager
        .snapshot(agent_id)
        .map(|snapshot| {
            snapshot
                .tabs
                .iter()
                .filter(|tab| !before.contains(&tab.id))
                .map(|tab| json!({ "tabId": tab.id, "url": tab.url, "active": tab.active }))
                .collect()
        })
        .unwrap_or_default()
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
        "browser.tab.switch" => {
            let id = text("tabId").ok_or("tabId is required")?;
            manager
                .switch_tab(app, agent_id, &id)
                .map_err(|error| error.to_string())?;
            read_state(&manager, agent_id, "changes", false).await
        }
        "browser.tab.close" => {
            let id = text("tabId").ok_or("tabId is required")?;
            manager
                .close_tab(app, agent_id, &id)
                .map_err(|error| error.to_string())?;
            Ok(tabs(&manager, agent_id))
        }
        "browser.navigate" => {
            let url = match (text("url"), text("go")) {
                (Some(_), Some(_)) => return Err("pass url or go, not both".into()),
                (None, Some(go)) => return step(app, agent_id, &go, params).await,
                (None, None) => return Err("url or go is required".into()),
                (Some(url), None) => url,
            };
            let url = match super::local_files::local_target(&url) {
                Some(path) => manager.local_files.url_for(&path)?,
                None => url,
            };
            let wanted = super::normalize_url(&url);
            let current = manager
                .active_view(agent_id)
                .ok()
                .and_then(|(tab_id, _)| manager.page(agent_id, &tab_id))
                .map(|page| page.url);
            let new_tab = params.get("newTab").and_then(Value::as_bool) == Some(true);
            let mut note = None;
            let tab_id = if new_tab || current.is_none() {
                manager
                    .open_tab(app, agent_id, Some(&url))
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                let current = current.unwrap_or_default();
                if current == wanted {
                    manager
                        .reload(agent_id)
                        .map_err(|error| error.to_string())?;
                    note = Some("the tab was already on this url, so it was reloaded");
                } else {
                    if same_document(&current, &wanted) {
                        note = Some("only the #fragment changed, so the page moved within itself and did not load again; use browser_navigate with go reload to load it afresh");
                    }
                    manager
                        .navigate(app, agent_id, &url)
                        .map_err(|error| error.to_string())?;
                }
                manager
                    .active_view(agent_id)
                    .map_err(|error| error.to_string())?
                    .0
            };
            tokio::time::sleep(SETTLE).await;
            let _ = manager
                .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                .await;
            let mut result = arrived(app, agent_id, &tab_id, params).await?;
            if let Some(note) = note {
                result["note"] = json!(note);
            }
            Ok(result)
        }
        "browser.state" => {
            if active_tab(&manager, agent_id).is_err() {
                return Ok(tabs(&manager, agent_id));
            }
            if let Some(selector) = text("selector") {
                let (_, view) = active(&manager, agent_id)?;
                return call(&view, "extract", &[json!(selector)]).await;
            }
            let full_text = params.get("fullText").and_then(Value::as_bool) == Some(true);
            read_state(&manager, agent_id, "full", full_text).await
        }
        "browser.find" => {
            let query = text("query").ok_or("query is required")?;
            let (_, view) = active(&manager, agent_id)?;
            call(&view, "find", &[json!(query), json!(text("role"))]).await
        }
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
            merge(result, report(&manager, agent_id, params).await?)
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
            merge(result, report(&manager, agent_id, params).await?)
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
                report(&manager, agent_id, params).await?,
            )
        }
        "browser.type" => {
            let value = text("text").ok_or("text is required")?;
            let submit = params
                .get("submit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let replace = params.get("replace").and_then(Value::as_bool) == Some(true);
            let (tab_id, view) = active(&manager, agent_id)?;
            let mut at = serde_json::Map::new();
            if let Some(index) = index("index") {
                at.insert("index".into(), json!(index));
            }
            if let Some(selector) = text("selector") {
                at.insert("selector".into(), json!(selector));
            }
            let at = if at.is_empty() {
                Value::Null
            } else {
                Value::Object(at)
            };
            let prepared =
                call(&view, "focus", &[at.clone(), json!(value), json!(replace)]).await?;
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
            let mut typed = merge(
                json!({
                    "typed": value.chars().count(),
                    "into": prepared.get("into").cloned().unwrap_or(Value::Null),
                    "replaced": replacing,
                    "submitted": submit,
                }),
                call(&view, "valueOf", &[at]).await?,
            )?;
            if let Some(warning) = typing_missed(&value, &typed) {
                typed["warning"] = json!(warning);
            }
            if submit {
                native::key(&view, "Enter").await?;
                settle(&manager, agent_id, &tab_id).await;
                return merge(typed, report(&manager, agent_id, params).await?);
            }
            Ok(typed)
        }
        "browser.press" => {
            let key = text("key").ok_or("key is required")?;
            let (tab_id, view) = active(&manager, agent_id)?;
            native::key(&view, &key).await?;
            settle(&manager, agent_id, &tab_id).await;
            merge(
                json!({ "pressed": key }),
                report(&manager, agent_id, params).await?,
            )
        }
        "browser.act" => act(app, agent_id, params).await,
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
            report(&manager, agent_id, params).await
        }
        "browser.scroll" => {
            let delta = params
                .get("deltaY")
                .and_then(Value::as_f64)
                .map(|delta| delta.clamp(-20_000.0, 20_000.0));
            let (_, view) = active(&manager, agent_id)?;
            call(
                &view,
                "scroll",
                &[
                    json!(delta),
                    element_target(params, "index"),
                    json!(text("to")),
                ],
            )
            .await
        }
        "browser.network" => {
            let since_current = match text("since").as_deref() {
                None => false,
                Some("navigation") => true,
                Some(_) => return Err("since must be \"navigation\"".into()),
            };
            let (tab_id, view) = active(&manager, agent_id)?;
            let passed = |key: &str| params.get(key).cloned().unwrap_or(Value::Null);
            let mut result = read_records(
                &view,
                "network",
                &[
                    passed("limit"),
                    passed("filter"),
                    passed("method"),
                    passed("status"),
                    passed("id"),
                ],
            )
            .await
            .unwrap_or_else(|error| json!({ "recording": false, "note": error }));
            if params.get("id").is_some() {
                return Ok(result);
            }
            let needle = text("filter").map(|filter| filter.to_lowercase());
            let documents: Vec<_> = manager
                .documents(&tab_id, since_current)
                .into_iter()
                .filter(|load| {
                    needle
                        .as_ref()
                        .is_none_or(|needle| load.url.to_lowercase().contains(needle))
                })
                .collect();
            if let Value::Object(map) = &mut result {
                map.insert("documents".into(), json!(documents));
            }
            Ok(result)
        }
        "browser.evaluate" => {
            let script = text("script").ok_or("script is required")?;
            let limit = Duration::from_millis(
                params
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(SCRIPT_TIMEOUT_MS)
                    .min(MAX_BLOCKING_MS),
            );
            let (_, view) = active(&manager, agent_id)?;
            let answer = match native::run_script(
                &view,
                &script_body(&format!("return ({script}\n);")),
                limit,
            )
            .await
            {
                Err(error) if is_parse_error(&error) => {
                    native::run_script(&view, &script_body(&script), limit).await
                }
                other => other,
            }
            .map_err(|error| script_failure(&error, limit))?;
            if answer.len() > MAX_SCRIPT_RESULT {
                let cut = (0..=MAX_SCRIPT_RESULT)
                    .rev()
                    .find(|at| answer.is_char_boundary(*at))
                    .unwrap_or(0);
                return Ok(json!({ "result": answer.get(..cut), "truncated": true }));
            }
            let result = serde_json::from_str::<Value>(&answer).unwrap_or(Value::String(answer));
            let mut answer = json!({ "result": result });
            if answer["result"].is_null() && !script.contains("return") && script.contains(';') {
                answer["note"] = json!(
                    "a script of several statements returns nothing unless it ends with return"
                );
            }
            Ok(answer)
        }
        "browser.console" => {
            let (_, view) = active(&manager, agent_id)?;
            read_records(
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
        "browser.screenshot" => {
            let (tab_id, view) = active(&manager, agent_id)?;
            let annotate = params.get("annotate").and_then(Value::as_bool) == Some(true);
            let full_page = params.get("fullPage").and_then(Value::as_bool) == Some(true);
            let named = element_target(params, "index");
            let area = if named == Value::Null {
                None
            } else {
                if full_page {
                    return Err("pass fullPage or an element, not both".into());
                }
                Some(call(&view, "areaOf", &[named]).await?)
            };
            let marked = if annotate {
                let page = state(&manager, agent_id).await?;
                let shown = call(&view, "showMarks", &[json!(true)]).await?;
                Some(marked_elements(&page, &shown))
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
            let image = screenshot(&view, height, area.as_ref().map(area_rect)).await;
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
            if let Some(area) = area {
                result["element"] = json!({ "index": area["index"], "label": area["label"] });
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
            let (tab_id, _) = active(&manager, agent_id)?;
            let Some(condition) = wait_condition(params) else {
                let ms = params
                    .get("ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(1000)
                    .min(MAX_WAIT_MS);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let _ = manager
                    .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                    .await;
                return report(&manager, agent_id, params).await;
            };
            let limit = params
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS)
                .min(MAX_BLOCKING_MS);
            let waited =
                wait_for(&manager, agent_id, &condition, Duration::from_millis(limit)).await;
            merge(waited, report(&manager, agent_id, params).await?)
        }
        "browser.viewport" => {
            let (tab_id, _) = active_tab(&manager, agent_id)?;
            let size = |key: &str| {
                params
                    .get(key)
                    .and_then(Value::as_u64)
                    .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
            };
            let fixed = match (text("preset").as_deref(), size("width"), size("height")) {
                (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                    return Err("pass a preset, or width and height, not both".into())
                }
                (Some("fit"), None, None) => Some(None),
                (Some(name), None, None) => Some(Some(
                    Viewport::preset(name)
                        .ok_or("preset must be desktop, tablet, mobile or fit")?,
                )),
                (None, Some(width), Some(height)) => Some(Some(Viewport::sized(width, height)?)),
                (None, None, None) => None,
                (None, _, _) => return Err("pass both width and height".into()),
            };
            if let Some(fixed) = fixed {
                manager
                    .set_viewport(agent_id, &tab_id, fixed)
                    .map_err(|error| error.to_string())?;
                let _ = manager
                    .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                    .await;
                manager.settle_viewport(agent_id, &tab_id).await?;
            }
            state(&manager, agent_id).await
        }
        _ => Err("unknown browser method".into()),
    }
}

/// Back, forward or reload in the current tab, then the page as it arrives.
async fn step(app: &AppHandle, agent_id: &str, go: &str, params: &Value) -> Result<Value, String> {
    let manager = app.state::<BrowserManager>();
    let (tab_id, view) = active_tab(&manager, agent_id)?;
    let hard = params.get("hard").and_then(Value::as_bool) == Some(true);
    match go {
        "back" => manager
            .history(agent_id, -1)
            .map_err(|error| error.to_string())?,
        "forward" => manager
            .history(agent_id, 1)
            .map_err(|error| error.to_string())?,
        "reload" if hard => native::reload_from_origin(&view).await?,
        "reload" => manager
            .reload(agent_id)
            .map_err(|error| error.to_string())?,
        other => return Err(format!("go must be back, forward or reload, not {other}")),
    }
    settle(&manager, agent_id, &tab_id).await;
    arrived(app, agent_id, &tab_id, params).await
}

/// Plays the steps in order and stops at the first that fails or that takes
/// the tab somewhere else, since what came after was planned for the old page.
async fn act(app: &AppHandle, agent_id: &str, params: &Value) -> Result<Value, String> {
    let manager = app.state::<BrowserManager>();
    let steps = params
        .get("steps")
        .and_then(Value::as_array)
        .filter(|steps| !steps.is_empty())
        .ok_or("steps must list at least one action")?;
    let before = outcome(&manager, agent_id)?;
    let mut done = Vec::new();
    let mut stopped = None;
    for (number, step) in steps.iter().enumerate() {
        let (method, step_params) = step_call(step)?;
        match Box::pin(run(app, agent_id, method, &step_params)).await {
            Ok(result) => done.push(step_summary(step, result)),
            Err(error) => {
                stopped = Some(json!({ "step": number, "error": error }));
                break;
            }
        }
        let now = outcome(&manager, agent_id)?;
        if number + 1 < steps.len() && moved_on(&before, &now) {
            stopped = Some(json!({
                "step": number,
                "reason": "the page moved on, so the steps after it were not run",
            }));
            break;
        }
    }
    let mut result = json!({ "done": done });
    if let Some(stopped) = stopped {
        result["stopped"] = stopped;
    }
    merge(result, report(&manager, agent_id, params).await?)
}

fn step_call(step: &Value) -> Result<(&'static str, Value), String> {
    let method = match step.get("action").and_then(Value::as_str) {
        Some("click") => "browser.click",
        Some("type") => "browser.type",
        Some("press") => "browser.press",
        _ => return Err("each step's action must be click, type or press".into()),
    };
    let mut params = step.clone();
    if let Value::Object(map) = &mut params {
        map.remove("action");
        map.insert("report".into(), json!("outcome"));
    }
    Ok((method, params))
}

/// What one step did, without the page details every step would repeat.
fn step_summary(step: &Value, mut result: Value) -> Value {
    if let Value::Object(map) = &mut result {
        for key in ["tabId", "url", "title", "loading", "tabs"] {
            map.remove(key);
        }
        map.insert("action".into(), step["action"].clone());
    }
    result
}

fn moved_on(before: &Value, now: &Value) -> bool {
    now.get("dialog").is_some()
        || now["loading"] == json!(true)
        || before["tabId"] != now["tabId"]
        || before["url"] != now["url"]
}

/// Whether a script failed to parse as an expression, as opposed to throwing
/// a SyntaxError while it ran, which retrying as a function body would only
/// run a second time.
fn is_parse_error(error: &str) -> bool {
    error.contains("SyntaxError")
        && ![
            "JSON Parse error",
            "did not match the expected pattern",
            "Invalid regular expression",
            "is not a valid selector",
        ]
        .iter()
        .any(|runtime| error.contains(runtime))
}

/// A script error in words an agent can act on.
fn script_failure(error: &str, limit: Duration) -> String {
    if error.is_empty() {
        format!(
            "the script did not finish within {} seconds; to wait for the page, use browser_wait with text, selector or networkIdle instead of polling in a script",
            limit.as_secs()
        )
    } else if error.contains("no longer reachable") {
        "the page navigated or reloaded while the script ran, so its result was lost; to reload, use browser_navigate with go reload".into()
    } else {
        error.to_owned()
    }
}

fn same_document(current: &str, wanted: &str) -> bool {
    let without_fragment = |url: &str| url.split('#').next().unwrap_or(url).to_owned();
    wanted.contains('#') && without_fragment(current) == without_fragment(wanted)
}

/// The conditions a wait names, or None when it names only a time.
fn wait_condition(source: &Value) -> Option<Value> {
    let mut condition = serde_json::Map::new();
    for key in ["text", "textGone", "selector", "selectorGone", "url"] {
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
async fn wait_for(
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

async fn pending_requests(view: &Webview) -> Option<u64> {
    read_records(view, "pending", &[])
        .await
        .ok()
        .and_then(|count| count.as_u64())
}

/// What a navigation or reload ends with: any wait it asked for, the page,
/// and why the page is not what was asked for when the load failed or left
/// nothing to read yet.
async fn arrived(
    app: &AppHandle,
    agent_id: &str,
    tab_id: &str,
    params: &Value,
) -> Result<Value, String> {
    let manager = app.state::<BrowserManager>();
    let waited = match params.get("waitFor").and_then(wait_condition) {
        Some(condition) => {
            let limit = params["waitFor"]
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS)
                .min(MAX_BLOCKING_MS);
            Some(wait_for(&manager, agent_id, &condition, Duration::from_millis(limit)).await)
        }
        None => None,
    };
    let mut result = report(&manager, agent_id, params).await?;
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

/// Why a field's value after typing suggests the text did not land where it
/// was meant to, if it does.
fn typing_missed(typed: &str, result: &Value) -> Option<&'static str> {
    let wanted = typed.split_whitespace().collect::<Vec<_>>().join(" ");
    if wanted.is_empty() {
        return None;
    }
    let Some(value) = result.get("value").and_then(Value::as_str) else {
        return Some(
            "the field's value could not be read, so check the page to see where the text went",
        );
    };
    if result.get("valueLength").is_some() && value.starts_with('\u{2022}') {
        return None;
    }
    let chars: Vec<char> = wanted.chars().collect();
    let ending: String = chars[chars.len().saturating_sub(40)..].iter().collect();
    if value.is_empty() {
        Some("the field is still empty, so the text went nowhere; the page may have moved focus, or the element is not a text field")
    } else if !value.contains(&ending) {
        Some("the field does not hold what was typed; the page may have reformatted it, or the text landed somewhere else")
    } else if result.get("replaced") == Some(&json!(false))
        && chars.len() >= 8
        && value.matches(ending.as_str()).count() > 1
    {
        Some("what was typed now appears more than once; pass replace: true to type over the field's text")
    } else {
        None
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

/// Where to point: the centre of the element a call names by number, `text`
/// or `selector`, scrolled into view, or CSS pixel coordinates in the tab's
/// viewport along with what lies under them.
async fn target(
    view: &Webview,
    params: &Value,
    index_key: &str,
    x_key: &str,
    y_key: &str,
) -> Result<(f64, f64, Value), String> {
    let expected = params.get("expectLabel").cloned().unwrap_or(Value::Null);
    let named = element_target(params, index_key);
    if named != Value::Null {
        return point(view, named, expected).await;
    }
    match (
        params.get(x_key).and_then(Value::as_f64),
        params.get(y_key).and_then(Value::as_f64),
    ) {
        (Some(x), Some(y)) => {
            let hit = call(view, "hitAt", &[json!(x), json!(y), expected]).await?;
            Ok((x, y, hit))
        }
        _ => Err(format!(
            "pass {index_key}, text or selector, or both {x_key} and {y_key}"
        )),
    }
}

/// The element a call names, as the page script's `resolve` takes it, or
/// null when it names none and points by coordinates instead.
fn element_target(params: &Value, index_key: &str) -> Value {
    let mut named = serde_json::Map::new();
    if let Some(index) = params.get(index_key).and_then(Value::as_u64) {
        named.insert("index".into(), json!(index));
    }
    if index_key == "index" {
        for key in ["text", "role", "selector"] {
            if let Some(value) = params.get(key).and_then(Value::as_str) {
                named.insert(key.into(), json!(value));
            }
        }
        if !named.contains_key("text") {
            named.remove("role");
        }
    }
    if named.is_empty() {
        Value::Null
    } else {
        Value::Object(named)
    }
}

/// The centre of the element `named` picks out, refused when `expected` no
/// longer matches its label.
async fn point(view: &Webview, named: Value, expected: Value) -> Result<(f64, f64, Value), String> {
    let point = call(view, "point", &[named, expected]).await?;
    let coordinate = |key: &str| {
        point
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| "the page returned no position".to_string())
    };
    Ok((coordinate("x")?, coordinate("y")?, point))
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
        use super::super::super::macos::World;
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

        /// An empty error means the script ran out of time.
        pub async fn run_script(
            view: &Webview,
            body: &str,
            limit: std::time::Duration,
        ) -> Result<String, String> {
            run_in(view, body, World::Page, limit)
                .await
                .map_err(Option::unwrap_or_default)
        }

        pub async fn hold_person_focus(view: &Webview) -> Result<(), String> {
            on_tab(view, input::hold_person_focus).await
        }

        pub async fn return_person_focus(view: &Webview) -> Result<(), String> {
            on_tab(view, input::return_person_focus).await
        }

        pub async fn probe_responsiveness(view: &Webview) -> Result<(), String> {
            on_tab(view, input::probe_responsiveness).await
        }

        pub async fn reload_from_origin(view: &Webview) -> Result<(), String> {
            on_tab(view, |tab| {
                // SAFETY: `on_tab` hands over the tab's WKWebView from inside
                // `with_webview`, on the main thread, while the view is alive.
                let webview = unsafe { &*tab.cast::<objc2_web_kit::WKWebView>() };
                // SAFETY: main thread, and `webview` outlives this call.
                let _ = unsafe { webview.reloadFromOrigin() };
                Ok(())
            })
            .await
        }

        pub async fn run_helper(view: &Webview, body: &str) -> Result<String, String> {
            match run_in(view, body, World::Helper, super::super::EVAL_TIMEOUT).await {
                Err(None) => Err("the page took too long to answer".into()),
                other => other.map_err(Option::unwrap_or_default),
            }
        }

        /// `Err(None)` when the script ran out of time.
        async fn run_in(
            view: &Webview,
            body: &str,
            world: World,
            limit: std::time::Duration,
        ) -> Result<String, Option<String>> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let body = body.to_owned();
            view.with_webview(move |platform| {
                super::super::super::macos::call_async(
                    platform.inner(),
                    &body,
                    world,
                    Box::new(move |result| {
                        let _ = sender.send(result);
                    }),
                );
            })
            .map_err(|error| Some(error.to_string()))?;
            let mut receiver = receiver;
            match tokio::time::timeout(super::super::SLOW_ANSWER.min(limit), &mut receiver).await {
                Ok(Ok(result)) => return result.map_err(Some),
                Ok(Err(_)) => return Err(Some("the tab went away".into())),
                Err(_) => {
                    let _ = super::probe_responsiveness(view).await;
                }
            }
            match tokio::time::timeout(limit.saturating_sub(super::super::SLOW_ANSWER), receiver)
                .await
            {
                Ok(Ok(result)) => result.map_err(Some),
                Ok(Err(_)) => Err(Some("the tab went away".into())),
                Err(_) => Err(None),
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

        pub async fn run_script(
            _: &Webview,
            _: &str,
            _: std::time::Duration,
        ) -> Result<String, String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn reload_from_origin(_: &Webview) -> Result<(), String> {
            Err(UNSUPPORTED.into())
        }

        pub async fn hold_person_focus(_: &Webview) -> Result<(), String> {
            Ok(())
        }

        pub async fn return_person_focus(_: &Webview) -> Result<(), String> {
            Ok(())
        }

        pub async fn probe_responsiveness(_: &Webview) -> Result<(), String> {
            Ok(())
        }

        pub async fn run_helper(_: &Webview, _: &str) -> Result<String, String> {
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

    pub async fn run_script(
        view: &Webview,
        body: &str,
        limit: std::time::Duration,
    ) -> Result<String, String> {
        platform::run_script(view, body, limit).await
    }

    /// Notes where the person's keyboard is before the agent sends input, so
    /// `return_person_focus` can put it back once the tab has taken it.
    pub async fn hold_person_focus(view: &Webview) -> Result<(), String> {
        platform::hold_person_focus(view).await
    }

    pub async fn return_person_focus(view: &Webview) -> Result<(), String> {
        platform::return_person_focus(view).await
    }

    /// Nudges WebKit into checking whether the tab's page still answers.
    pub async fn probe_responsiveness(view: &Webview) -> Result<(), String> {
        platform::probe_responsiveness(view).await
    }

    /// Reloads skipping the cache, so a stale script or stylesheet is fetched again.
    pub async fn reload_from_origin(view: &Webview) -> Result<(), String> {
        platform::reload_from_origin(view).await
    }

    /// Runs `body` where the page's scripts cannot reach it.
    pub async fn run_helper(view: &Webview, body: &str) -> Result<String, String> {
        platform::run_helper(view, body).await
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
    if let Some(stall) = manager.stalled(&tab_id) {
        return Err(stall.message().into());
    }
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
        .map_err(|_| {
            "no browser tab is open; call browser_navigate first. Tabs do not outlive Sikemux, so a restart closes them".to_string()
        })
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
    read_state(manager, agent_id, "full", false).await
}

/// What an action hands back about the page, chosen by its `report`: only the
/// outcome, what changed since the last read (the default), or the full state.
async fn report(manager: &BrowserManager, agent_id: &str, params: &Value) -> Result<Value, String> {
    match params.get("report").and_then(Value::as_str) {
        Some("outcome") => outcome(manager, agent_id),
        Some("full") => state(manager, agent_id).await,
        _ => read_state(manager, agent_id, "changes", false).await,
    }
}

fn outcome(manager: &BrowserManager, agent_id: &str) -> Result<Value, String> {
    let (tab_id, _) = active_tab(manager, agent_id)?;
    let page = manager.page(agent_id, &tab_id).unwrap_or_default();
    let mut result = json!({
        "tabId": tab_id,
        "url": page.url,
        "title": page.title,
        "loading": page.loading,
    });
    if let Some(dialog) = manager.dialog(&tab_id) {
        result["dialog"] = json!(dialog);
    }
    Ok(result)
}

/// The element lines that got a box on the picture, in the page's order.
fn marked_elements(page: &Value, shown: &Value) -> Value {
    let marked: Vec<String> = shown
        .get("marked")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .map(|id| format!("[{id}] "))
        .collect();
    let lines = page
        .get("elements")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            marked
                .iter()
                .any(|prefix| line.starts_with(prefix.as_str()))
        })
        .collect::<Vec<_>>();
    json!(lines.join("\n"))
}

async fn read_state(
    manager: &BrowserManager,
    agent_id: &str,
    mode: &str,
    full_text: bool,
) -> Result<Value, String> {
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
        call(&view, "state", &[json!(mode), json!(full_text)]).await?
    };
    if let Some(viewport) = result.get_mut("viewport").and_then(Value::as_object_mut) {
        let fixed = manager.viewport(agent_id, &tab_id);
        let narrow = fixed.is_none()
            && viewport
                .get("width")
                .and_then(Value::as_u64)
                .is_some_and(|width| width < NARROW_VIEWPORT);
        viewport.insert(
            "fixed".into(),
            fixed.map_or(json!(false), |fixed| json!(fixed)),
        );
        if narrow {
            viewport.insert(
                "note".into(),
                json!("the pane is narrow, so the page may show its phone layout; browser_viewport with preset desktop lays it out wide"),
            );
        }
    }
    if let Value::Object(map) = &mut result {
        map.insert("tabId".into(), json!(tab_id));
        map.insert("loading".into(), json!(page.loading));
        map.insert("visible".into(), json!(manager.shown(agent_id, &tab_id)));
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
    let answer = answer_of("window.__sikemux", function, args);
    let raw = native::run_helper(view, &format!("{PAGE_SCRIPT}\nreturn {answer};")).await?;
    let inner = serde_json::from_str::<Value>(&raw)
        .map_err(|_| "the page returned an unreadable answer".to_string())?;
    unwrap_answer(inner)
}

/// Read what the recorder kept for `function` ("network" or "console").
async fn read_records(view: &Webview, function: &str, args: &[Value]) -> Result<Value, String> {
    let raw = eval(view, &answer_of(RECORDS_SCRIPT.trim(), function, args)).await?;
    let outer: Value =
        serde_json::from_str(&raw).map_err(|_| "the page returned no answer".to_string())?;
    let inner = match outer {
        Value::String(text) => serde_json::from_str::<Value>(&text)
            .map_err(|_| "the page returned an unreadable answer".to_string())?,
        other => other,
    };
    unwrap_answer(inner)
}

fn answer_of(target: &str, function: &str, args: &[Value]) -> String {
    let args = args
        .iter()
        .map(|arg| serde_json::to_string(arg).unwrap_or_else(|_| "null".into()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "JSON.stringify((() => {{ try {{ return {{ ok: ({target}).{function}({args}) }}; }} catch (error) {{ return {{ error: String((error && error.message) || error) }}; }} }})())"
    )
}

fn unwrap_answer(inner: Value) -> Result<Value, String> {
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
        Ok(_) => Err("the page did not answer, so it may not have loaded; browser_network's documents show whether its load failed".into()),
        Err(_) => Err("the page took too long to answer".into()),
    }
}

fn area_rect(area: &Value) -> (f64, f64, f64, f64) {
    let side = |key: &str| area.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    (side("left"), side("top"), side("width"), side("height"))
}

/// The visible part of the tab, only `area` of it, or with `height` the
/// whole page down to it.
async fn screenshot(
    view: &Webview,
    height: Option<f64>,
    area: Option<(f64, f64, f64, f64)>,
) -> Result<Vec<u8>, String> {
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
                None => super::macos::snapshot_jpeg(platform.inner(), area, done),
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
        let _ = (view, height, area);
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
    fn a_step_becomes_its_tool_call_and_reports_only_what_it_did() {
        let (method, params) =
            step_call(&json!({ "action": "type", "index": 3, "text": "hi" })).unwrap();
        assert_eq!(method, "browser.type");
        assert_eq!(
            params,
            json!({ "index": 3, "text": "hi", "report": "outcome" })
        );
        assert!(step_call(&json!({ "action": "act" })).is_err());
        let summary = step_summary(
            &json!({ "action": "press", "key": "Enter" }),
            json!({ "pressed": "Enter", "url": "https://a.test", "tabId": "t", "loading": false }),
        );
        assert_eq!(summary, json!({ "pressed": "Enter", "action": "press" }));
    }

    #[test]
    fn steps_stop_once_the_tab_is_somewhere_else() {
        let page = json!({ "tabId": "t", "url": "https://a.test/", "loading": false });
        assert!(!moved_on(&page, &page));
        let navigated = json!({ "tabId": "t", "url": "https://a.test/next", "loading": false });
        assert!(moved_on(&page, &navigated));
        let loading = json!({ "tabId": "t", "url": "https://a.test/", "loading": true });
        assert!(moved_on(&page, &loading));
        let asking =
            json!({ "tabId": "t", "url": "https://a.test/", "loading": false, "dialog": {} });
        assert!(moved_on(&page, &asking));
    }

    #[test]
    fn a_call_names_its_element_by_number_text_or_selector() {
        assert_eq!(
            element_target(&json!({ "text": "Save", "role": "button" }), "index"),
            json!({ "text": "Save", "role": "button" })
        );
        assert_eq!(
            element_target(&json!({ "selector": "#go", "role": "button" }), "index"),
            json!({ "selector": "#go" })
        );
        assert_eq!(
            element_target(&json!({ "x": 3, "y": 4 }), "index"),
            Value::Null
        );
        assert_eq!(
            element_target(&json!({ "fromIndex": 2, "text": "ignored" }), "fromIndex"),
            json!({ "index": 2 })
        );
    }

    #[test]
    fn typing_that_went_nowhere_or_doubled_up_is_called_out() {
        let after = |value: &str, replaced: bool| json!({ "value": value, "replaced": replaced });
        assert!(typing_missed("hello", &after("hello", true)).is_none());
        assert!(typing_missed("hello", &after("", true))
            .unwrap()
            .contains("still empty"));
        assert!(typing_missed("hello", &after("goodbye", true))
            .unwrap()
            .contains("does not hold"));
        assert!(
            typing_missed("adjunctive", &after("adjunctiveadjunctive", false))
                .unwrap()
                .contains("replace: true")
        );
        assert!(typing_missed("", &after("", true)).is_none());
        assert!(typing_missed(
            "secret",
            &json!({ "value": "\u{2022}\u{2022}", "valueLength": 6 })
        )
        .is_none());
    }

    #[test]
    fn only_a_script_that_failed_to_parse_is_run_again_as_statements() {
        assert!(is_parse_error("SyntaxError: Unexpected keyword 'const'"));
        assert!(!is_parse_error(
            "SyntaxError: JSON Parse error: Unexpected identifier"
        ));
        assert!(!is_parse_error(
            "SyntaxError: The string did not match the expected pattern."
        ));
        assert!(!is_parse_error("TypeError: null is not an object"));
    }

    #[test]
    fn script_failures_say_what_to_do_instead() {
        let limit = Duration::from_secs(30);
        assert!(script_failure("", limit).contains("browser_wait"));
        assert!(script_failure(
            "Completion handler for function call is no longer reachable",
            limit
        )
        .contains("go reload"));
        assert_eq!(script_failure("TypeError: x", limit), "TypeError: x");
    }

    #[test]
    fn a_fragment_change_is_told_apart_from_a_new_page() {
        assert!(same_document(
            "https://a.test/doc",
            "https://a.test/doc#size"
        ));
        assert!(same_document(
            "https://a.test/doc#a",
            "https://a.test/doc#b"
        ));
        assert!(!same_document(
            "https://a.test/doc",
            "https://a.test/other#size"
        ));
        assert!(!same_document("https://a.test/doc#a", "https://a.test/doc"));
    }

    #[test]
    fn a_wait_names_its_conditions_or_only_a_time() {
        assert_eq!(wait_condition(&json!({ "ms": 500 })), None);
        assert_eq!(
            wait_condition(&json!({ "text": "Saved", "networkIdle": true, "timeoutMs": 5 })),
            Some(json!({ "text": "Saved", "networkIdle": true }))
        );
        assert_eq!(wait_condition(&json!({ "networkIdle": false })), None);
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

    #[test]
    fn an_annotated_picture_lists_only_the_elements_it_boxed() {
        let page =
            json!({ "elements": "[1] <a> Home (/)\n[12] <button> Save\n[2] <button> Hidden" });
        let listed = marked_elements(&page, &json!({ "marked": [12, 1] }));
        assert_eq!(listed, json!("[1] <a> Home (/)\n[12] <button> Save"));
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

    /// The page script runs in a world of its own, which cannot see the
    /// globals the recorder leaves among the page's scripts.
    #[test]
    fn only_the_records_script_reads_the_recorder() {
        for global in ["__sikemuxNet", "__sikemuxConsole"] {
            assert!(!PAGE_SCRIPT.contains(global), "page.js reads {global}");
            assert!(
                RECORDS_SCRIPT.contains(global),
                "records.js misses {global}"
            );
        }
        assert!(
            answer_of(RECORDS_SCRIPT.trim(), "network", &[json!(5)]).contains("})).network(5) }")
        );
    }
}
