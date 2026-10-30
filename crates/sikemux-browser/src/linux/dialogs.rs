//! A page's alerts, confirms and prompts on Linux: the person sees each in a dialog of the
//! app's own, and an agent sees it too and can answer it, as on macOS. Everything here
//! runs on the GTK thread that owns the page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gtk::prelude::*;
use webkit2gtk::{ScriptDialog, ScriptDialogType, WebView, WebViewExt};

use crate::burst::Burst;
use crate::{PageDialog, TabEvents};

thread_local! {
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::new(HashMap::new());
}

/// A dialog the page waits on, until the person or an agent answers it.
struct Open {
    page: ScriptDialog,
    shown: gtk::MessageDialog,
    field: Option<gtk::Entry>,
    events: Arc<dyn TabEvents>,
}

pub(crate) fn attach(webview: &WebView, tab_id: &str, events: Arc<dyn TabEvents>) {
    let flood = RefCell::new(Burst::new(crate::DIALOG_LIMIT, crate::DIALOG_WINDOW));
    let (asking, told) = (tab_id.to_owned(), events);
    webview.connect_script_dialog(move |view, dialog| {
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
        present(view, dialog, &asking, told.clone());
        true
    });
    let gone = tab_id.to_owned();
    webview.connect_destroy(move |_| finish(&gone, false, None));
}

fn present(view: &WebView, dialog: &ScriptDialog, tab_id: &str, events: Arc<dyn TabEvents>) {
    let kind = dialog.dialog_type();
    let message = dialog.message().map(String::from).unwrap_or_default();
    let default_text = (kind == ScriptDialogType::Prompt).then(|| {
        dialog
            .prompt_get_default_text()
            .map(String::from)
            .unwrap_or_default()
    });
    let host = view
        .uri()
        .and_then(|uri| url::Url::parse(&uri).ok())
        .and_then(|url| url.host_str().map(str::to_owned))
        .filter(|host| !host.is_empty());
    let title = match host {
        Some(host) => format!("{host} says"),
        None => "This page says".to_owned(),
    };
    let parent = view
        .toplevel()
        .and_then(|toplevel| toplevel.downcast::<gtk::Window>().ok());
    let shown = gtk::MessageDialog::new(
        parent.as_ref(),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Question,
        gtk::ButtonsType::None,
        &title,
    );
    shown.set_secondary_text(Some(&message));
    if kind != ScriptDialogType::Alert {
        shown.add_button("Cancel", gtk::ResponseType::Cancel);
    }
    shown.add_button("OK", gtk::ResponseType::Ok);
    shown.set_default_response(gtk::ResponseType::Ok);
    let field = default_text.as_ref().map(|text| {
        let field = gtk::Entry::new();
        field.set_text(text);
        field.set_activates_default(true);
        if let Ok(area) = shown.message_area().downcast::<gtk::Box>() {
            area.pack_start(&field, false, false, 0);
        }
        field
    });
    let answering = tab_id.to_owned();
    shown.connect_response(move |_, response| {
        finish(&answering, response == gtk::ResponseType::Ok, None);
    });
    events.dialog(Some(PageDialog {
        kind: match kind {
            ScriptDialogType::Confirm => "confirm",
            ScriptDialogType::Prompt => "prompt",
            _ => "alert",
        },
        message,
        default_text,
    }));
    shown.show_all();
    OPEN.with(|open| {
        open.borrow_mut().insert(
            tab_id.to_owned(),
            Open {
                page: dialog.clone(),
                shown,
                field,
                events,
            },
        )
    });
}

/// Ends the tab's open dialog as if the person pressed OK or Cancel, typing `text` into a
/// prompt first.
pub fn answer(tab_id: &str, accept: bool, text: Option<&str>) -> Result<(), String> {
    if !OPEN.with(|open| open.borrow().contains_key(tab_id)) {
        return Err("this tab has no open dialog".into());
    }
    finish(tab_id, accept, text);
    Ok(())
}

/// Gives the page its answer, once, and takes the dialog away.
fn finish(tab_id: &str, accept: bool, text: Option<&str>) {
    let Some(open) = OPEN.with(|open| open.borrow_mut().remove(tab_id)) else {
        return;
    };
    match open.page.dialog_type() {
        ScriptDialogType::Confirm => open.page.confirm_set_confirmed(accept),
        ScriptDialogType::Prompt if accept => {
            let typed = match (text, &open.field) {
                (Some(text), _) => text.to_owned(),
                (None, Some(field)) => field.text().into(),
                (None, None) => String::new(),
            };
            open.page.prompt_set_text(&typed);
        }
        _ => {}
    }
    open.page.close();
    // SAFETY: the dialog is ours alone, out of the open set, and nothing uses it after this.
    unsafe { open.shown.destroy() };
    open.events.dialog(None);
}
