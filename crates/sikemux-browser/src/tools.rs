//! The agent's browser tools, answered over the harness socket. Every method
//! acts on the agent's own tabs, the same ones the person sees in the pane.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;

mod target;
mod wait;

use target::*;
use wait::*;

use crate::manager::BrowserManager;
use crate::native::{self, Mouse};
use crate::viewport::Viewport;
use crate::{Tab, BLANK_URL};
use sikemux_base::cli::protocol::{HarnessRequest, PAGE_REPLY_TIMEOUT};

use crate::native::EVAL_TIMEOUT;
use crate::{PAGE_SCRIPT, RECORDS_SCRIPT};
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_millis(250);
const DOWNLOAD_WAIT: Duration = Duration::from_secs(15);
const MAX_WAIT_MS: u64 = 30_000;
const SCRIPT_TIMEOUT_MS: u64 = 30_000;
/// Well inside the time the core waits for any reply.
const MAX_BLOCKING_MS: u64 = (PAGE_REPLY_TIMEOUT.as_secs() - 5) * 1000;
const MAX_SCRIPT_RESULT: usize = 100_000;
/// A full-page capture goes through a PDF page, and PDF stops at 14,400 points.
const MAX_PAGE_HEIGHT: f64 = 14_400.0;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(3);
const UPLOAD_LANDING: Duration = Duration::from_secs(2);
/// How long a click waits for the element it names to show up.
const APPEAR_WAIT: Duration = Duration::from_secs(3);
/// A browser_act call stops starting steps in time to answer within the core's wait.
const ACT_TIME: Duration = Duration::from_millis(MAX_BLOCKING_MS - 10_000);
/// What a step may take at worst: a click waiting for its element and then a load.
const STEP_RESERVE: Duration = Duration::from_secs(LOAD_TIMEOUT.as_secs() + APPEAR_WAIT.as_secs());
const UPLOAD_POLL: Duration = Duration::from_millis(50);
const MAX_UPLOAD_FILES: usize = 20;
const DRAG_STEPS: u32 = 12;
const DRAG_STEP_DELAY: Duration = Duration::from_millis(16);
const NARROW_VIEWPORT: u64 = 700;
const PAGE_VIEWPORT_HEIGHT: u64 = 600;
const PAGE_SCRIPT_LIMIT: Duration = Duration::from_secs(20);

/// A write a page is about to make to a site, put to the core before it goes:
/// what it does and the site's origin. Its error is what the agent is told.
pub type CheckWrite = dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<(), String>>>>;

/// Runs on a CLI broker thread, so blocking on the async runtime is safe.
pub fn execute(
    manager: &BrowserManager,
    request: &HarnessRequest,
    check: &CheckWrite,
) -> Result<Value, String> {
    let agent_id = request
        .agent_id
        .as_deref()
        .ok_or("browser tools need the agent's id")?;
    let acts_on_a_tab = !matches!(
        request.method.as_str(),
        "browser.tab.close" | "browser.page"
    );
    let mut marks = Vec::new();
    if acts_on_a_tab {
        manager.announce_acting(agent_id);
        let reloading = manager.reloading(agent_id);
        marks.extend(manager.mark_acting(agent_id));
        if reloading {
            wait_for_reload(manager, agent_id);
        }
    }
    let before = tab_ids(manager, agent_id);
    let download_mark = manager.download_mark(agent_id);
    let mut result = sikemux_base::runtime::block_on(run(
        manager,
        agent_id,
        check,
        &request.method,
        &request.params,
    ))
    .map_err(|error| error.to_string());
    if acts_on_a_tab {
        marks.extend(manager.mark_acting(agent_id));
        manager.release_acting(agent_id, marks);
    }
    if let Ok(Value::Object(map)) = &mut result {
        if request.method != "browser.navigate" {
            let opened = opened_tabs(manager, agent_id, &before);
            if !opened.is_empty() {
                map.insert("openedTabs".into(), json!(opened));
            }
        }
        let saved = saved_files(manager, agent_id, download_mark);
        if !saved.is_empty() {
            map.entry("downloads").or_insert(json!(saved));
        }
    }
    result
}

/// Files the action made the page download, given a moment to finish so the
/// agent gets a path it can use straight away.
fn saved_files(manager: &BrowserManager, agent_id: &str, mark: u64) -> Vec<Value> {
    manager
        .settled_downloads_since(agent_id, mark, DOWNLOAD_WAIT)
        .into_iter()
        .map(|file| json!(file))
        .collect()
}

/// An unloaded tab loads its page again before the agent's tools touch it.
fn wait_for_reload(manager: &BrowserManager, agent_id: &str) {
    if let Ok((tab_id, _)) = manager.active_view(agent_id) {
        sikemux_base::runtime::block_on(manager.wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT));
    }
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
    manager: &BrowserManager,
    agent_id: &str,
    check: &CheckWrite,
    method: &str,
    params: &Value,
) -> Result<Value, String> {
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
                .switch_tab(agent_id, &id)
                .map_err(|error| error.to_string())?;
            if manager.reloading(agent_id) {
                let _ = manager.active_view(agent_id);
                manager.wait_until_loaded(agent_id, &id, LOAD_TIMEOUT).await;
            }
            read_state(manager, agent_id, "changes", false).await
        }
        "browser.tab.close" => {
            let id = text("tabId").ok_or("tabId is required")?;
            manager
                .close_tab(agent_id, &id)
                .map_err(|error| error.to_string())?;
            Ok(tabs(manager, agent_id))
        }
        "browser.navigate" => {
            let url = match (text("url"), text("go")) {
                (Some(_), Some(_)) => return Err("pass url or go, not both".into()),
                (None, Some(go)) => return step(manager, agent_id, &go, params).await,
                (None, None) => return Err("url or go is required".into()),
                (Some(url), None) => url,
            };
            let url = manager.page_url(&url).map_err(|error| error.to_string())?;
            let wanted = url.clone();
            let current = manager
                .active_view(agent_id)
                .ok()
                .and_then(|(tab_id, _)| manager.page(agent_id, &tab_id))
                .map(|page| page.url);
            let mut new_tab = params.get("newTab").and_then(Value::as_bool) == Some(true);
            let started = Instant::now();
            let mut note = None;
            let mut current = current;
            if new_tab {
                if let Some(tab_id) = manager.same_site_tab(agent_id, &url) {
                    manager
                        .switch_tab(agent_id, &tab_id)
                        .map_err(|error| error.to_string())?;
                    current = manager.page(agent_id, &tab_id).map(|page| page.url);
                    new_tab = false;
                    note = Some("your tab on this site was used rather than opening another; close tabs you are done with");
                }
            }
            let mut closed = None;
            let tab_id = if new_tab || current.is_none() {
                closed = manager.make_room(agent_id);
                manager
                    .open_tab(agent_id, Some(&url))
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                if let Ok((tab_id, _)) = manager.active_view(agent_id) {
                    crate::person::leave_be(&tab_id)?;
                    if !crate::address::same_site(current.as_deref().unwrap_or_default(), &wanted) {
                        manager.hand_back(agent_id, &tab_id);
                    }
                }
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
                        .navigate(agent_id, &url)
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
            let mut result = arrived(manager, agent_id, &tab_id, params, started).await?;
            if let Some(note) = note {
                result["note"] = json!(note);
            }
            if let Some(closed) = closed {
                result["closedTab"] = json!(format!(
                    "you had {} tabs of your own, so the one unused longest, {closed}, was closed",
                    crate::manager::AGENT_TAB_LIMIT
                ));
            }
            Ok(result)
        }
        "browser.state" => {
            if active_tab(manager, agent_id).is_err() {
                return Ok(tabs(manager, agent_id));
            }
            if let Some(selector) = text("selector") {
                let (_, view) = active(manager, agent_id)?;
                return call(&view, "extract", &[json!(selector)]).await;
            }
            let full_text = params.get("fullText").and_then(Value::as_bool) == Some(true);
            let mut result = read_state(manager, agent_id, "full", full_text).await?;
            let downloads = manager.recent_downloads(agent_id);
            if !downloads.is_empty() {
                result["downloads"] = json!(downloads);
            }
            Ok(result)
        }
        "browser.find" => {
            let query = text("query").ok_or("query is required")?;
            let (_, view) = active(manager, agent_id)?;
            call(&view, "find", &[json!(query), json!(text("role"))]).await
        }
        "browser.click" => {
            let (tab_id, view) = active(manager, agent_id)?;
            let (x, y, mut result) = target(&view, params, "index", "x", "y").await?;
            let hover = params.get("hover").and_then(Value::as_bool) == Some(true);
            let clicks = if params.get("double").and_then(Value::as_bool) == Some(true) {
                2
            } else {
                1
            };
            if !hover {
                let write = call(&view, "writeAt", &[json!(x), json!(y)]).await?;
                put_to_person(write, check).await?;
            }
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
            settle(manager, agent_id, &tab_id).await;
            merge(result, report(manager, agent_id, params).await?)
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
            let (tab_id, view) = active(manager, agent_id)?;
            let by_point = params.get("x").is_some() || params.get("y").is_some();
            let input = match by_point {
                true => Value::Null,
                false => call(&view, "fileInput", &[element_target(params, "index")]).await?,
            };
            let placed = input.is_object();
            let (x, y, mut result) = match input.get("x").and_then(Value::as_f64) {
                Some(x) => (x, input["y"].as_f64().unwrap_or_default(), input),
                None => target(&view, params, "index", "x", "y").await?,
            };
            let names: Vec<String> = paths
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            call(&view, "uploadLanded", &[json!(true)]).await?;
            manager.offer_upload(&tab_id, paths);
            show_pointer(&view, x, y, true).await;
            native::mouse(&view, Mouse::Down, x, y, 1).await?;
            native::mouse(&view, Mouse::Up, x, y, 1).await?;
            let started = Instant::now();
            let mut opened = true;
            while manager.upload_pending(&tab_id) {
                if started.elapsed() >= UPLOAD_TIMEOUT {
                    opened = false;
                    break;
                }
                tokio::time::sleep(UPLOAD_POLL).await;
            }
            let landed_by = Instant::now() + UPLOAD_LANDING;
            let mut landed = false;
            while opened && !landed && Instant::now() < landed_by {
                tokio::time::sleep(UPLOAD_POLL).await;
                landed = call(&view, "uploadLanded", &[json!(false)]).await? == json!(true);
            }
            manager.withdraw_upload(&tab_id);
            if placed {
                call(&view, "unplaceFileInput", &[]).await?;
            }
            if !opened {
                return Err("that did not open a file chooser; pass the selector or number of a file input, or of the button that opens one".into());
            }
            if !landed {
                return Err("a file chooser took the files, but no file input on the page changed, so the page did not get them; pass the selector or number of the button or drop zone that opens its chooser instead".into());
            }
            result["uploaded"] = json!(names);
            settle(manager, agent_id, &tab_id).await;
            merge(result, report(manager, agent_id, params).await?)
        }
        "browser.drag" => {
            let (tab_id, view) = active(manager, agent_id)?;
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
            settle(manager, agent_id, &tab_id).await;
            merge(
                json!({ "from": from, "to": to, "dragged": dragged }),
                report(manager, agent_id, params).await?,
            )
        }
        "browser.type" => {
            let value = text("text").ok_or("text is required")?;
            let submit = params
                .get("submit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let replace = params.get("replace").and_then(Value::as_bool) == Some(true);
            let (tab_id, view) = active(manager, agent_id)?;
            let mut at = serde_json::Map::new();
            if let Some(index) = index("index") {
                at.insert("index".into(), json!(index));
            }
            if let Some(selector) = text("selector") {
                at.insert("selector".into(), json!(selector));
            }
            if let Some(field) = text("field") {
                at.insert("text".into(), json!(field));
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
            if submit {
                let write = call(&view, "writeOfField", std::slice::from_ref(&at)).await?;
                put_to_person(write, check).await?;
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
                settle(manager, agent_id, &tab_id).await;
                return merge(typed, report(manager, agent_id, params).await?);
            }
            Ok(typed)
        }
        "browser.press" => {
            let key = text("key").ok_or("key is required")?;
            let (tab_id, view) = active(manager, agent_id)?;
            if plain_enter(&key) {
                let write = call(&view, "writeOfField", &[Value::Null]).await?;
                put_to_person(write, check).await?;
            }
            native::key(&view, &key).await?;
            settle(manager, agent_id, &tab_id).await;
            merge(
                json!({ "pressed": key }),
                report(manager, agent_id, params).await?,
            )
        }
        "browser.act" => act(manager, agent_id, check, params).await,
        "browser.dialog" => {
            let accept = params
                .get("accept")
                .and_then(Value::as_bool)
                .ok_or("accept is required")?;
            let (tab_id, view) = active_tab(manager, agent_id)?;
            if manager.dialog(&tab_id).is_none() {
                return Err("no dialog is open in the current tab".into());
            }
            native::answer_dialog(&view, &tab_id, accept, text("text")).await?;
            settle(manager, agent_id, &tab_id).await;
            report(manager, agent_id, params).await
        }
        "browser.scroll" => {
            let delta = params
                .get("deltaY")
                .and_then(Value::as_f64)
                .map(|delta| delta.clamp(-20_000.0, 20_000.0));
            let (_, view) = active(manager, agent_id)?;
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
            let (tab_id, view) = active(manager, agent_id)?;
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
            let started = Instant::now();
            let (tab_id, view) = active(manager, agent_id)?;
            manager
                .wait_until_loaded(agent_id, &tab_id, LOAD_TIMEOUT)
                .await;
            let limit = Duration::from_millis(
                params
                    .get("timeoutMs")
                    .and_then(Value::as_u64)
                    .unwrap_or(SCRIPT_TIMEOUT_MS)
                    .min(MAX_BLOCKING_MS),
            )
            .min(Duration::from_millis(MAX_BLOCKING_MS).saturating_sub(started.elapsed()));
            let mut allowed: Vec<String> =
                serde_json::from_value(params["allowedSites"].clone()).unwrap_or_default();
            loop {
                let left = limit.saturating_sub(started.elapsed());
                let answer = run_agent_script(&view, &script, left, Some(&allowed)).await?;
                let write = &answer["result"]["sikemuxHeldWrite"];
                if !write.is_object() {
                    return Ok(answer);
                }
                let origin = write["origin"].as_str().unwrap_or_default().to_owned();
                if allowed.contains(&origin) {
                    return Err(format!("the script kept writing to {origin} while held"));
                }
                put_to_person(write.clone(), check).await?;
                allowed.push(origin);
            }
        }
        "browser.console" => {
            let (_, view) = active(manager, agent_id)?;
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
            let held = text("viewport")
                .map(|name| {
                    Viewport::preset(&name)
                        .and_then(|preset| Viewport::sized(preset.width, preset.height).ok())
                        .ok_or_else(|| "viewport must be desktop, tablet or mobile".to_owned())
                })
                .transpose()?;
            if let Some(fixed) = held {
                let (tab_id, _) = active(manager, agent_id)?;
                manager
                    .resize_viewport(agent_id, &tab_id, fixed)
                    .map_err(|error| error.to_string())?;
                manager.settle_viewport(agent_id, &tab_id).await?;
            }
            let (tab_id, view) = active(manager, agent_id)?;
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
                let page = state(manager, agent_id).await?;
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
                "data": base64::engine::general_purpose::STANDARD.encode(&image?),
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
            if let Some(fixed) = held {
                result["viewport"] = json!(fixed);
            }
            Ok(result)
        }
        "browser.record" => match text("action").as_deref() {
            Some("start") => crate::recording::start(manager, agent_id, text("path")).await,
            Some("stop") => crate::recording::stop(manager, agent_id).await,
            _ => Err("action must be start or stop".into()),
        },
        "browser.annotate" => {
            let (_, view) = active(manager, agent_id)?;
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
            let (tab_id, _) = active(manager, agent_id)?;
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
                let mut result = report(manager, agent_id, params).await?;
                result["note"] = json!("a fixed wait; text, textGone, selector, url, urlGone or networkIdle wait only as long as it takes");
                return Ok(result);
            };
            let limit = params
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS)
                .min(MAX_BLOCKING_MS);
            let waited =
                wait_for(manager, agent_id, &condition, Duration::from_millis(limit)).await;
            merge(waited, report(manager, agent_id, params).await?)
        }
        "browser.viewport" => {
            let (tab_id, view) = active_tab(manager, agent_id)?;
            let size = |key: &str| {
                params
                    .get(key)
                    .and_then(Value::as_u64)
                    .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
            };
            let (mut width, mut height) = (size("width"), size("height"));
            if width.is_some() != height.is_some() && text("preset").is_none() {
                let now = call(&view, "viewportSize", &[]).await?;
                let current = |key: &str| now[key].as_u64().map(|value| value as u32);
                width = width.or_else(|| current("width"));
                height = height.or_else(|| current("height"));
            }
            let fixed = match (text("preset").as_deref(), width, height) {
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
            let (_, view) = active(manager, agent_id)?;
            let mut size = call(&view, "viewportSize", &[]).await?;
            size["tabId"] = json!(tab_id);
            size["held"] = json!(manager.viewport(agent_id, &tab_id));
            Ok(size)
        }
        "browser.page" => {
            let source = std::path::PathBuf::from(text("path").ok_or("path is required")?);
            if !source.is_absolute() {
                return Err(format!("{} is not an absolute path", source.display()));
            }
            let width = params
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(crate::REPLY_COLUMN_WIDTH as u64);
            let html = tokio::task::spawn_blocking(move || sikemux_base::pages::build(&source))
                .await
                .map_err(|error| error.to_string())??;
            let url = manager.host().page_url(agent_id, &html)?;
            let mut preview = crate::preview::Preview::open(
                manager.host(),
                agent_id,
                &url,
                (width as f64, PAGE_VIEWPORT_HEIGHT as f64),
            )?;
            preview.loaded(LOAD_TIMEOUT).await?;
            tokio::time::sleep(SETTLE).await;
            let view = preview.view();
            let ran = match text("script") {
                Some(script) => {
                    let ran = run_agent_script(view, &script, PAGE_SCRIPT_LIMIT, None).await?;
                    tokio::time::sleep(SETTLE).await;
                    Some(ran)
                }
                None => None,
            };
            let content_height = call(view, "contentHeight", &[]).await?;
            let console = read_records(view, "console", &[Value::Null, json!(false)]).await?;
            let mut result = json!({
                "width": width,
                "contentHeight": content_height,
                "console": console,
            });
            if let Some(ran) = ran {
                result["script"] = ran;
            }
            if params.get("picture").and_then(Value::as_bool) == Some(false) {
                return Ok(result);
            }
            let page_height = call(view, "pageHeight", &[])
                .await?
                .as_f64()
                .ok_or("the page did not report its height")?;
            let image = screenshot(view, Some(page_height), None).await?;
            result["mimeType"] = json!("image/jpeg");
            result["data"] = json!(base64::engine::general_purpose::STANDARD.encode(&image));
            if page_height > MAX_PAGE_HEIGHT {
                result["cutAt"] = json!(MAX_PAGE_HEIGHT);
            }
            Ok(result)
        }
        _ => Err("unknown browser method".into()),
    }
}

/// Runs an agent's script in `view`, an expression or statements ending in
/// return, and answers with what it returned as JSON.
async fn run_agent_script(
    view: &Tab,
    script: &str,
    limit: Duration,
    allowed_sites: Option<&[String]>,
) -> Result<Value, String> {
    let body = match native::run_helper(view, &shape_check(script))
        .await?
        .as_str()
    {
        "expression" => script_body(&format!("return ({script}\n);"), allowed_sites),
        "statements" => script_body(script, allowed_sites),
        error => return Err(error.to_owned()),
    };
    let answer = native::run_script(view, &body, limit)
        .await
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
        answer["note"] =
            json!("a script of several statements returns nothing unless it ends with return");
    }
    Ok(answer)
}

/// Back, forward or reload in the current tab, then the page as it arrives.
async fn step(
    manager: &BrowserManager,
    agent_id: &str,
    go: &str,
    params: &Value,
) -> Result<Value, String> {
    let (tab_id, view) = active_tab(manager, agent_id)?;
    crate::person::leave_be(&tab_id)?;
    let started = Instant::now();
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
    settle(manager, agent_id, &tab_id).await;
    arrived(manager, agent_id, &tab_id, params, started).await
}

/// Plays the steps in order and stops at the first that fails, or once the
/// page moves on when a later step names an element by number or point,
/// since those belong to the old page.
async fn act(
    manager: &BrowserManager,
    agent_id: &str,
    check: &CheckWrite,
    params: &Value,
) -> Result<Value, String> {
    let steps = params
        .get("steps")
        .and_then(Value::as_array)
        .filter(|steps| !steps.is_empty())
        .ok_or("steps must list at least one action")?;
    let calls = steps.iter().map(step_call).collect::<Result<Vec<_>, _>>()?;
    let deadline = Instant::now() + ACT_TIME;
    let mut done = Vec::new();
    let mut stopped = None;
    for (number, (step, (method, mut step_params))) in steps.iter().zip(calls).enumerate() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left < STEP_RESERVE {
            stopped = Some(json!({
                "step": number,
                "reason": "the call ran out of time, so this step and the ones after it were not run; send them again",
            }));
            break;
        }
        if method == "browser.wait" {
            let key = if step_params.get("ms").is_some() {
                "ms"
            } else {
                "timeoutMs"
            };
            let asked = step_params
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONDITION_WAIT_MS);
            step_params[key] = json!(asked.min(left.as_millis() as u64));
        }
        let before = outcome(manager, agent_id).ok();
        let failed = match Box::pin(run(manager, agent_id, check, method, &step_params)).await {
            Ok(result) => {
                let failed = step_failure(&result);
                done.push(step_summary(step, result));
                failed
            }
            Err(error) => Some(error),
        };
        if let Some(error) = failed {
            stopped = Some(json!({ "step": number, "action": step["action"], "error": error }));
            break;
        }
        let rest = &steps[number + 1..];
        if let Some(reason) = before
            .zip(outcome(manager, agent_id).ok())
            .and_then(|(before, now)| blocks_the_rest(&before, &now, rest))
        {
            if !rest.is_empty() {
                stopped = Some(json!({ "step": number, "reason": reason }));
                break;
            }
        }
    }
    let mut result = json!({ "done": done });
    let finished = stopped.is_none();
    if let Some(stopped) = stopped {
        result["stopped"] = stopped;
    }
    let reads = steps
        .iter()
        .any(|step| matches!(step["action"].as_str(), Some("read" | "find")));
    let mut report_params = params.clone();
    if finished && reads && params.get("report").is_none() {
        report_params["report"] = json!("outcome");
    }
    merge(result, report(manager, agent_id, &report_params).await?)
}

fn step_call(step: &Value) -> Result<(&'static str, Value), String> {
    let method = match step.get("action").and_then(Value::as_str) {
        Some("click") => "browser.click",
        Some("type") => "browser.type",
        Some("press") => "browser.press",
        Some("navigate") => "browser.navigate",
        Some("wait") => "browser.wait",
        Some("find") => "browser.find",
        Some("read") => "browser.state",
        _ => {
            return Err(
                "each step's action must be click, type, press, navigate, wait, find or read"
                    .into(),
            )
        }
    };
    let mut params = step.clone();
    if let Value::Object(map) = &mut params {
        map.remove("action");
        map.insert("report".into(), json!("outcome"));
        if method == "browser.wait" && wait_condition(step).is_none() {
            let pause = map.remove("timeoutMs").unwrap_or(json!(1000));
            map.insert("ms".into(), pause);
        }
    }
    Ok((method, params))
}

/// Why a step that answered still did not do what it was for: a wait that ran
/// out, or a page that failed to load.
fn step_failure(result: &Value) -> Option<String> {
    if result["met"] == json!(false) {
        let failing = result["failing"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        return Some(format!(
            "waited {} ms, and still {failing}",
            result["waitedMs"]
        ));
    }
    result["loadError"].as_str().map(str::to_owned)
}

/// What one step did, without the page details every step would repeat.
fn step_summary(step: &Value, mut result: Value) -> Value {
    if let Value::Object(map) = &mut result {
        for key in ["tabId", "url", "title", "loading", "tabs", "visible"] {
            map.remove(key);
        }
        map.insert("action".into(), step["action"].clone());
    }
    result
}

/// Why the steps after this one cannot run on the page it left the tab on.
fn blocks_the_rest(before: &Value, now: &Value, rest: &[Value]) -> Option<&'static str> {
    if now.get("dialog").is_some() {
        return Some(
            "a dialog opened, so the steps after it were not run; answer it with browser_dialog",
        );
    }
    if before["tabId"] != now["tabId"] {
        return Some("another tab came to the front, so the steps after it were not run");
    }
    let moved = now["loading"] == json!(true) || before["url"] != now["url"];
    let by_place = rest.iter().any(|step| {
        ["index", "x", "y"]
            .iter()
            .any(|key| step.get(*key).is_some())
    });
    (moved && by_place).then_some(
        "the page moved on, and a later step names an element by number or point from the old page, so the steps after it were not run",
    )
}

/// Whether `script` reads as an expression or as statements, or the syntax error that
/// stops both. It only parses, in the helper world, where the page's content policy
/// can't forbid it, so the page never runs a script that fails to parse: WebKit
/// crashed on macOS 26 while describing that error.
fn shape_check(script: &str) -> String {
    let source = serde_json::to_string(script).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"const AsyncFunction = (async () => {{}}).constructor;
const source = {source};
try {{ new AsyncFunction("return (" + source + "\n);"); return "expression"; }} catch {{}}
try {{ new AsyncFunction(source); return "statements"; }}
catch (error) {{ return String(error); }}"#
    )
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
fn script_body(script: &str, allowed_sites: Option<&[String]>) -> String {
    let allowed = allowed_sites.map(|sites| serde_json::to_string(sites).unwrap_or_default());
    let watches = allowed.is_some();
    let (watch, release) = match watches {
        true => (
            format!(
                "window.__sikemuxNet?.watch({});",
                allowed.unwrap_or_default()
            ),
            r#"const held = window.__sikemuxNet?.release();
if (held) return JSON.stringify({ sikemuxHeldWrite: held });"#
                .to_owned(),
        ),
        false => (String::new(), String::new()),
    };
    format!(
        r#"let value, thrown, failed = false;
{watch}
try {{
value = await (async () => {{
{script}
}})();
}} catch (error) {{ thrown = error; failed = true; }}
{release}
if (failed) throw thrown;
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

/// Puts `write`, what the page script found, to the core, and fails with what the agent is
/// told when it is held. A null `write` is one the page does not make.
async fn put_to_person(write: Value, check: &CheckWrite) -> Result<(), String> {
    if write.is_null() {
        return Ok(());
    }
    let method = write["method"].as_str().unwrap_or_default();
    let origin = write["origin"].as_str().unwrap_or_default().to_owned();
    let path = write["path"].as_str().unwrap_or_default();
    check(format!("{method} {origin}{path}"), origin).await
}

/// Whether `key` is Enter with nothing else held, which sends the form a field is in.
fn plain_enter(key: &str) -> bool {
    use crate::keys::{parse, Key, Modifiers, Named};
    parse(key).is_ok_and(|chord| {
        chord.key == Key::Named(Named::Enter) && chord.modifiers == Modifiers::default()
    })
}

/// The tab the agent is on, unless a dialog has frozen its page: a page
/// waiting on an alert answers nothing, so calling into it would only time out.
pub(crate) fn active(manager: &BrowserManager, agent_id: &str) -> Result<(String, Tab), String> {
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

fn active_tab(manager: &BrowserManager, agent_id: &str) -> Result<(String, Tab), String> {
    manager
        .active_view(agent_id)
        .map_err(|_| {
            "no browser tab is open; call browser_navigate first. Quitting Sikemux closes tabs; an update's restart keeps them, but each page loads again from its address".to_string()
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
pub(crate) async fn call(view: &Tab, function: &str, args: &[Value]) -> Result<Value, String> {
    let answer = answer_of("window.__sikemux", function, args);
    let raw = native::run_helper(view, &format!("{PAGE_SCRIPT}\nreturn {answer};")).await?;
    let inner = serde_json::from_str::<Value>(&raw)
        .map_err(|_| "the page returned an unreadable answer".to_string())?;
    unwrap_answer(inner)
}

/// Read what the recorder kept for `function` ("network" or "console").
async fn read_records(view: &Tab, function: &str, args: &[Value]) -> Result<Value, String> {
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

pub(crate) async fn eval(view: &Tab, script: &str) -> Result<String, String> {
    match tokio::time::timeout(EVAL_TIMEOUT, view.eval(script)).await {
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
pub(crate) async fn screenshot(
    view: &Tab,
    height: Option<f64>,
    area: Option<(f64, f64, f64, f64)>,
) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "linux")]
    use crate::linux::page;
    #[cfg(target_os = "macos")]
    use crate::macos as page;

    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = std::sync::Mutex::new(Some(sender));
    view.run(move |webview| {
        let done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> = Box::new(move |result| {
            if let Some(sender) = sender.locked().take() {
                let _ = sender.send(result);
            }
        });
        match height {
            Some(height) => page::full_page_jpeg(webview, height, MAX_PAGE_HEIGHT, done),
            None => page::snapshot_jpeg(webview, area, done),
        }
    });
    match tokio::time::timeout(EVAL_TIMEOUT, receiver).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("screenshot was abandoned".into()),
        Err(_) => Err("screenshot took too long".into()),
    }
}

impl BrowserManager {
    pub fn page(&self, agent_id: &str, tab_id: &str) -> Option<super::TabPage> {
        self.agents
            .locked()
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
    fn an_annotated_picture_lists_only_the_elements_it_boxed() {
        let page =
            json!({ "elements": "[1] <a> Home (/)\n[12] <button> Save\n[2] <button> Hidden" });
        let listed = marked_elements(&page, &json!({ "marked": [12, 1] }));
        assert_eq!(listed, json!("[1] <a> Home (/)\n[12] <button> Save"));
    }

    /// A dispatched click is untrusted, and pages that check refuse it.
    #[test]
    fn clicks_and_keys_are_never_played_by_the_page_script() {
        for synthetic in [
            r#"Event("click""#,
            r#"Event("mousedown""#,
            r#"Event("keydown""#,
        ] {
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
