//! What reaches a tab's page the way a person's input does, as trusted events,
//! and scripts run inside it. Each engine gives the same few primitives.

use std::time::Duration;

#[cfg(target_os = "linux")]
use crate::linux::dialogs::answer as answer_on_page;
#[cfg(target_os = "linux")]
use crate::linux::input;
#[cfg(target_os = "linux")]
use crate::linux::page::{call_async, World};
#[cfg(target_os = "macos")]
use crate::macos::{answer_dialog as answer_on_page, call_async, World};
use crate::Tab;

#[cfg(target_os = "macos")]
use crate::input;

pub use input::Mouse;

pub(crate) const EVAL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a page may take to answer before WebKit is asked whether it hangs.
const SLOW_ANSWER: Duration = Duration::from_secs(1);

#[cfg(target_os = "macos")]
type PageView = objc2_web_kit::WKWebView;
#[cfg(target_os = "linux")]
type PageView = webkit2gtk::WebView;

/// Runs `act` against the tab's native view on the thread that owns it.
async fn on_tab<T: Send + 'static>(
    view: &Tab,
    act: impl FnOnce(&PageView) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match tokio::time::timeout(EVAL_TIMEOUT, view.ask(act)).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(error),
        Err(_) => Err("the tab took too long to answer".into()),
    }
}

pub async fn mouse(view: &Tab, kind: Mouse, x: f64, y: f64, clicks: isize) -> Result<(), String> {
    on_tab(view, move |tab| input::mouse(tab, kind, x, y, clicks)).await
}

#[cfg(target_os = "macos")]
pub async fn key(view: &Tab, name: &str) -> Result<(), String> {
    let stroke = input::parse_key(name)?;
    on_tab(view, move |tab| input::key(tab, &stroke)).await
}

#[cfg(target_os = "linux")]
pub async fn key(view: &Tab, name: &str) -> Result<(), String> {
    crate::keys::parse(name)?;
    let name = name.to_owned();
    on_tab(view, move |tab| input::key(tab, &name)).await
}

pub async fn insert_text(view: &Tab, text: &str) -> Result<(), String> {
    let text = text.to_owned();
    on_tab(view, move |tab| input::insert_text(tab, &text)).await
}

/// Reloads skipping the cache, so a stale script or stylesheet is fetched again.
#[cfg(target_os = "macos")]
pub async fn reload_from_origin(view: &Tab) -> Result<(), String> {
    on_tab(view, |webview| {
        // SAFETY: `on_tab` runs this on the main thread with the live webview.
        let _ = unsafe { webview.reloadFromOrigin() };
        Ok(())
    })
    .await
}

/// Reloads skipping the cache, so a stale script or stylesheet is fetched again.
#[cfg(target_os = "linux")]
pub async fn reload_from_origin(view: &Tab) -> Result<(), String> {
    on_tab(view, |webview| {
        webkit2gtk::WebViewExt::reload_bypass_cache(webview);
        Ok(())
    })
    .await
}

pub async fn answer_dialog(
    view: &Tab,
    tab_id: &str,
    accept: bool,
    text: Option<String>,
) -> Result<(), String> {
    let tab_id = tab_id.to_owned();
    on_tab(view, move |_| {
        answer_on_page(&tab_id, accept, text.as_deref())
    })
    .await
}

/// An empty error means the script ran out of time.
pub async fn run_script(view: &Tab, body: &str, limit: Duration) -> Result<String, String> {
    run_in(view, body, World::Page, limit)
        .await
        .map_err(Option::unwrap_or_default)
}

/// Runs `body` in the tab's helper world, which sees the page's DOM but none
/// of its scripts.
pub(crate) async fn run_helper(view: &Tab, body: &str) -> Result<String, String> {
    match run_in(view, body, World::Helper, EVAL_TIMEOUT).await {
        Err(None) => Err("the page took too long to answer".into()),
        other => other.map_err(Option::unwrap_or_default),
    }
}

/// `Err(None)` when the script ran out of time.
async fn run_in(
    view: &Tab,
    body: &str,
    world: World,
    limit: Duration,
) -> Result<String, Option<String>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let body = body.to_owned();
    view.run(move |webview| {
        call_async(
            webview,
            &body,
            world,
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
    });
    let mut receiver = receiver;
    match tokio::time::timeout(SLOW_ANSWER.min(limit), &mut receiver).await {
        Ok(Ok(result)) => return result.map_err(Some),
        Ok(Err(_)) => return Err(Some("the tab went away".into())),
        Err(_) => {
            // Nudges WebKit into checking whether the tab's page still answers.
            let _ = on_tab(view, input::probe_responsiveness).await;
        }
    }
    match tokio::time::timeout(limit.saturating_sub(SLOW_ANSWER), receiver).await {
        Ok(Ok(result)) => result.map_err(Some),
        Ok(Err(_)) => Err(Some("the tab went away".into())),
        Err(_) => Err(None),
    }
}
