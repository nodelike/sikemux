//! Page dialogs on Linux held in Rust without OS modal popups, answered by the agent or
//! the inline tab UI. Everything here runs on the GTK thread that owns the page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gtk::prelude::*;
use webkit2gtk::{
    FileChooserRequest, FileChooserRequestExt, ScriptDialog, ScriptDialogType, WebView, WebViewExt,
};

use crate::burst::Burst;
use crate::{PageDialog, TabEvents};

thread_local! {
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::new(HashMap::new());
}

struct Open {
    page: ScriptDialog,
    default_text: Option<String>,
    events: Arc<dyn TabEvents>,
}

pub(crate) fn attach(webview: &WebView, tab_id: &str, events: Arc<dyn TabEvents>) {
    let flood = RefCell::new(Burst::new(crate::DIALOG_LIMIT, crate::DIALOG_WINDOW));
    let uploads = events.clone();
    let (asking, told) = (tab_id.to_owned(), events);
    webview.connect_script_dialog(move |_view, dialog| {
        match dialog.dialog_type() {
            ScriptDialogType::Alert | ScriptDialogType::Confirm | ScriptDialogType::Prompt => {}
            // WebKit on macOS leaves a page without asking, and so does this.
            ScriptDialogType::BeforeUnloadConfirm => {
                dialog.confirm_set_confirmed(true);
                return true;
            }
            _ => return false,
        }
        if !flood.borrow_mut().admit(Instant::now()) {
            dialog.confirm_set_confirmed(false);
            return true;
        }
        hold(dialog, &asking, told.clone());
        true
    });
    let gone = tab_id.to_owned();
    webview.connect_destroy(move |_| finish(&gone, false, None));
    webview.connect_run_file_chooser(move |_view, request| choose_files(request, &uploads));
}

/// Hands the page the files an agent offered through `browser_upload`. With none offered,
/// WebKit shows its own chooser for the person.
fn choose_files(request: &FileChooserRequest, events: &Arc<dyn TabEvents>) -> bool {
    let Some(mut paths) = events.upload() else {
        return false;
    };
    if !request.selects_multiple() {
        paths.truncate(1);
    }
    let paths: Vec<String> = (paths.iter())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    request.select_files(&paths);
    true
}

fn hold(dialog: &ScriptDialog, tab_id: &str, events: Arc<dyn TabEvents>) {
    let kind = dialog.dialog_type();
    let message = dialog.message().map(String::from).unwrap_or_default();
    let default_text = (kind == ScriptDialogType::Prompt).then(|| {
        dialog
            .prompt_get_default_text()
            .map(String::from)
            .unwrap_or_default()
    });
    let described = PageDialog {
        kind: match kind {
            ScriptDialogType::Confirm => "confirm",
            ScriptDialogType::Prompt => "prompt",
            _ => "alert",
        },
        message,
        default_text: default_text.clone(),
    };
    let previous = OPEN.with(|open| {
        open.borrow_mut().insert(
            tab_id.to_owned(),
            Open {
                page: dialog.clone(),
                default_text,
                events: events.clone(),
            },
        )
    });
    if let Some(previous) = previous {
        previous.page.confirm_set_confirmed(false);
        previous.page.close();
    }
    events.dialog(Some(described));
}

pub fn answer(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    if !OPEN.with(|open| open.borrow().contains_key(tab_id)) {
        return Err("this tab has no open dialog".into());
    }
    finish(tab_id, accept, text);
    Ok(())
}

fn finish(tab_id: &str, accept: bool, text: Option<&str>) {
    let Some(open) = OPEN.with(|open| open.borrow_mut().remove(tab_id)) else {
        return;
    };
    match open.page.dialog_type() {
        ScriptDialogType::Confirm => open.page.confirm_set_confirmed(accept),
        ScriptDialogType::Prompt if accept => {
            let typed = text.or(open.default_text.as_deref()).unwrap_or_default();
            open.page.prompt_set_text(typed);
        }
        _ => {}
    }
    open.events.dialog(None);
    open.page.close();
}
