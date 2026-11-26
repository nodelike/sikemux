//! A page's alert, confirm and prompt wait, unseen, for the agent's answer, since the page
//! sits under the app's own drawing where a dialog of the system's would cover the window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{take_pwstr, ScriptDialogOpeningEventHandler};
use windows::core::{HSTRING, PWSTR};

use super::{ui, Page};
use crate::{PageDialog, TabEvents};

struct Open {
    args: ICoreWebView2ScriptDialogOpeningEventArgs,
    deferral: ICoreWebView2Deferral,
    events: Arc<dyn TabEvents>,
}

thread_local! {
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::new(HashMap::new());
}

pub(super) fn listen(page: &Page, tab_id: &str, events: Arc<dyn TabEvents>) -> Result<(), String> {
    let tab_id = tab_id.to_owned();
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
                let (mut message, mut default_text) = (PWSTR::null(), PWSTR::null());
                args.Message(&mut message)?;
                args.DefaultText(&mut default_text)?;
                let deferral = args.GetDeferral()?;
                let kind = match kind {
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => "confirm",
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => "prompt",
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_BEFOREUNLOAD => "confirm",
                    _ => "alert",
                };
                let default_text = take_pwstr(default_text);
                events.dialog(Some(PageDialog {
                    kind,
                    message: take_pwstr(message),
                    default_text: (kind == "prompt").then_some(default_text),
                }));
                OPEN.with_borrow_mut(|open| {
                    open.insert(
                        tab_id.clone(),
                        Open {
                            args,
                            deferral,
                            events: events.clone(),
                        },
                    )
                });
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
    // SAFETY: answers the live dialog on the window's thread, once.
    unsafe {
        if accept {
            if let Some(text) = text {
                let _ = open.args.SetResultText(&HSTRING::from(text));
            }
            let _ = open.args.Accept();
        }
        let _ = open.deferral.Complete();
    }
    open.events.dialog(None);
    Ok(())
}

pub(super) fn forget(tab_id: &str) {
    if let Some(open) = OPEN.with_borrow_mut(|open| open.remove(tab_id)) {
        // SAFETY: completes the deferred dialog on the window's thread when its tab closes.
        unsafe {
            let _ = open.deferral.Complete();
        }
        open.events.dialog(None);
    }
}
