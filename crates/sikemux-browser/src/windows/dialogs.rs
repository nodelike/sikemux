//! Page dialogs on Windows held in Rust without OS modal popups, answered by the agent or
//! the inline tab UI.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, ScriptDialogOpeningEventHandler};
use windows::core::{HSTRING, PWSTR};

use super::{ui, Page};
use crate::burst::Burst;
use crate::{PageDialog, TabEvents};

struct Open {
    args: ICoreWebView2ScriptDialogOpeningEventArgs,
    deferral: ICoreWebView2Deferral,
    default_text: Option<String>,
    events: Arc<dyn TabEvents>,
}

thread_local! {
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::new(HashMap::new());
}

pub(super) fn listen(page: &Page, tab_id: &str, events: Arc<dyn TabEvents>) -> Result<(), String> {
    let tab_id = tab_id.to_owned();
    let flood = RefCell::new(Burst::new(crate::DIALOG_LIMIT, crate::DIALOG_WINDOW));
    let mut token = 0;
    // SAFETY: registers a handler on the live webview, on the window's thread.
    unsafe {
        page.webview.add_ScriptDialogOpening(
            &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND::default();
                args.Kind(&mut kind)?;
                if kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD {
                    let _ = args.Accept();
                    return Ok(());
                }
                if !flood.borrow_mut().admit(Instant::now()) {
                    return Ok(());
                }
                let (mut message, mut default_text) = (PWSTR::null(), PWSTR::null());
                args.Message(&mut message)?;
                args.DefaultText(&mut default_text)?;
                let deferral = args.GetDeferral()?;
                let kind = match kind {
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => "confirm",
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => "prompt",
                    _ => "alert",
                };
                let default_text = take_pwstr(default_text);
                let default_text = (kind == "prompt").then_some(default_text);
                let described = PageDialog {
                    kind,
                    message: take_pwstr(message),
                    default_text: default_text.clone(),
                };
                let previous = OPEN.with_borrow_mut(|open| {
                    open.insert(
                        tab_id.clone(),
                        Open {
                            args,
                            deferral,
                            default_text,
                            events: events.clone(),
                        },
                    )
                });
                if let Some(previous) = previous {
                    let _ = previous.deferral.Complete();
                }
                events.dialog(Some(described));
                Ok(())
            })),
            &mut token,
        )
    }
    .map_err(|error| error.message())
}

pub fn answer(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    if !ui::here() {
        return Err("a dialog is answered on the window's thread".into());
    }
    let Some(open) = OPEN.with_borrow_mut(|open| open.remove(tab_id)) else {
        return Err("this tab has no open dialog".into());
    };
    open.events.dialog(None);
    // SAFETY: answers the live dialog on the window's thread, once.
    unsafe {
        if accept {
            if let Some(reply) = text.or(open.default_text.as_deref()) {
                let _ = open.args.SetResultText(&HSTRING::from(reply));
            }
            let _ = open.args.Accept();
        }
        let _ = open.deferral.Complete();
    }
    Ok(())
}

pub(super) fn forget(tab_id: &str) {
    if let Some(open) = OPEN.with_borrow_mut(|open| open.remove(tab_id)) {
        open.events.dialog(None);
        // SAFETY: completes the deferred dialog on the window's thread when its tab closes.
        unsafe {
            let _ = open.deferral.Complete();
        }
    }
}
