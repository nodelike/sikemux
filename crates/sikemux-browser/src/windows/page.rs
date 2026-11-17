//! The WebView2 side of a tab's page: its scripts, its pictures, its history and the app
//! chords pressed in it. Everything here runs on the window's thread.
//!
//! Scripts and pictures go through the page's DevTools methods, which await a script's promise,
//! run it in a world of its own and picture any part of the page.

use std::sync::OnceLock;

use base64::Engine;
use serde_json::{json, Value};
use sikemux_process::sync::Locked;
use webview2_com::AcceleratorKeyPressedEventHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::core::{Interface, HSTRING};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};

use super::{devtools, Page};
use crate::BrowserShortcut;

/// The world the agents' helpers run in: it shares the page's DOM but none of its
/// JavaScript, so the page cannot tamper with what runs there.
const HELPER_WORLD: &str = "sikemux";
const JPEG_QUALITY: u8 = 82;

type Shortcut = Box<dyn Fn(BrowserShortcut) + Send + Sync>;

static SHORTCUT: OnceLock<Shortcut> = OnceLock::new();

#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

pub(crate) fn install_shortcuts(send: impl Fn(BrowserShortcut) + Send + Sync + 'static) {
    let _ = SHORTCUT.set(Box::new(send));
}

/// Command chords pressed in the page go to the app's keymap, not the page.
pub(super) fn listen_for_shortcuts(
    page: &Page,
    agent_id: &str,
    tab_id: &str,
) -> Result<(), String> {
    let (agent_id, tab_id) = (agent_id.to_owned(), tab_id.to_owned());
    let mut token = 0;
    // SAFETY: registers a handler on the live controller, on the window's thread.
    unsafe {
        page.controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                    return Ok(());
                }
                let mut key = 0;
                args.VirtualKey(&mut key)?;
                let Some(shortcut) = chord_of(key) else {
                    return Ok(());
                };
                if let Some(send) = SHORTCUT.get() {
                    args.SetHandled(true)?;
                    send(BrowserShortcut {
                        agent_id: agent_id.clone(),
                        tab_id: tab_id.clone(),
                        ..shortcut
                    });
                }
                Ok(())
            })),
            &mut token,
        )
    }
    .map_err(|error| error.message())
}

/// Control with a letter, a digit or a sign, except the editing chords the page keeps.
fn chord_of(key: u32) -> Option<BrowserShortcut> {
    // SAFETY: reads the keyboard state of this thread's current message.
    let held = |key: u16| unsafe { GetKeyState(i32::from(key)) } < 0;
    if !held(VK_CONTROL.0) || held(VK_MENU.0) {
        return None;
    }
    let (key, code) = match key {
        0x41..=0x5A => {
            let letter = char::from(key as u8).to_ascii_lowercase();
            (letter, format!("Key{}", letter.to_ascii_uppercase()))
        }
        0x30..=0x39 => {
            let digit = char::from(key as u8);
            (digit, format!("Digit{digit}"))
        }
        0xDB => ('[', "BracketLeft".into()),
        0xDD => (']', "BracketRight".into()),
        0xBC => (',', "Comma".into()),
        0xBE => ('.', "Period".into()),
        0xBF => ('/', "Slash".into()),
        0xBA => (';', "Semicolon".into()),
        0xDE => ('\'', "Quote".into()),
        0xDC => ('\\', "Backslash".into()),
        0xBD => ('-', "Minus".into()),
        0xBB => ('=', "Equal".into()),
        0xC0 => ('`', "Backquote".into()),
        _ => return None,
    };
    if matches!(key, 'a' | 'c' | 'v' | 'x' | 'z' | 'y') {
        return None;
    }
    Some(BrowserShortcut {
        agent_id: String::new(),
        tab_id: String::new(),
        key: key.to_string(),
        code,
        shift: held(VK_SHIFT.0),
        alt: false,
    })
}

/// Runs `body` as the body of an async function in the page, awaiting any promise it
/// returns. The body must return a string.
pub fn call_async(
    page: &Page,
    body: &str,
    world: World,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let expression = format!("(async () => {{\n{body}\n}})()");
    match world {
        World::Page => evaluate(page, expression, None, done),
        World::Helper => {
            let page_now = page.clone();
            helper_world(page, move |context| match context {
                Ok(context) => evaluate(&page_now, expression, Some(context), done),
                Err(why) => done(Err(why)),
            });
        }
    }
}

fn evaluate(
    page: &Page,
    expression: String,
    context: Option<i64>,
    done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    let mut params = json!({
        "expression": expression,
        "awaitPromise": true,
        "returnByValue": true,
    });
    if let Some(context) = context {
        params["contextId"] = json!(context);
    }
    devtools(page, "Runtime.evaluate", &params, move |answer| {
        done(answer.and_then(|answer| {
            if let Some(thrown) = answer.get("exceptionDetails") {
                return Err(thrown
                    .pointer("/exception/description")
                    .or_else(|| thrown.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("the script threw")
                    .to_owned());
            }
            answer
                .pointer("/result/value")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| "the script returned nothing readable".into())
        }))
    });
}

/// The page's helper world, made once for each document it shows.
fn helper_world(page: &Page, then: impl FnOnce(Result<i64, String>) + 'static) {
    let page_now = page.clone();
    devtools(page, "Page.getFrameTree", &json!({}), move |tree| {
        let tree = match tree {
            Ok(tree) => tree,
            Err(why) => return then(Err(why)),
        };
        let frame = tree.pointer("/frameTree/frame");
        let field = |name: &str| {
            frame
                .and_then(|frame| frame.get(name))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let (frame, document) = (field("id"), field("loaderId"));
        let made = page_now.helper.locked().clone();
        if let Some((made_for, context)) = made.filter(|(made_for, _)| *made_for == document) {
            let _ = made_for;
            return then(Ok(context));
        }
        let helper = page_now.helper.clone();
        devtools(
            &page_now,
            "Page.createIsolatedWorld",
            &json!({ "frameId": frame, "worldName": HELPER_WORLD }),
            move |world| {
                then(world.and_then(|world| {
                    let context = world
                        .get("executionContextId")
                        .and_then(Value::as_i64)
                        .ok_or("the page gave the helpers no world")?;
                    *helper.locked() = Some((document, context));
                    Ok(context)
                }))
            },
        );
    });
}

/// The visible part of the page, or only `area` of it in CSS pixels.
pub fn snapshot_jpeg(
    page: &Page,
    area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let mut params = json!({ "format": "jpeg", "quality": JPEG_QUALITY });
    if let Some((x, y, width, height)) = area {
        params["clip"] = json!({ "x": x, "y": y, "width": width, "height": height, "scale": 1 });
    }
    devtools(page, "Page.captureScreenshot", &params, move |picture| {
        done(picture.and_then(decoded))
    });
}

/// The whole page down to `height` CSS pixels, at most `most` of them.
pub fn full_page_jpeg(
    page: &Page,
    height: f64,
    most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    let page_now = page.clone();
    devtools(page, "Page.getLayoutMetrics", &json!({}), move |metrics| {
        let width = match metrics {
            Ok(metrics) => metrics
                .pointer("/cssLayoutViewport/clientWidth")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            Err(why) => return done(Err(why)),
        };
        let params = json!({
            "format": "jpeg",
            "quality": JPEG_QUALITY,
            "captureBeyondViewport": true,
            "clip": { "x": 0, "y": 0, "width": width, "height": height.min(most), "scale": 1 },
        });
        devtools(
            &page_now,
            "Page.captureScreenshot",
            &params,
            move |picture| done(picture.and_then(decoded)),
        );
    });
}

fn decoded(picture: Value) -> Result<Vec<u8>, String> {
    let data = picture
        .get("data")
        .and_then(Value::as_str)
        .ok_or("the page could not be captured")?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "could not read the page image".into())
}

pub fn history(page: &Page, delta: i32) {
    // SAFETY: on the window's thread, with the page's live webview.
    let _ = unsafe {
        if delta < 0 {
            page.webview.GoBack()
        } else {
            page.webview.GoForward()
        }
    };
}

/// An empty agent leaves the engine's own, which names the Edge it runs on.
pub fn introduce_as(page: &Page, agent: &str) {
    if agent.is_empty() {
        return;
    }
    // SAFETY: on the window's thread, with the page's live webview.
    unsafe {
        if let Ok(settings) = page
            .webview
            .Settings()
            .and_then(|settings| settings.cast::<ICoreWebView2Settings2>())
        {
            let _ = settings.SetUserAgent(&HSTRING::from(agent));
        }
    }
}
