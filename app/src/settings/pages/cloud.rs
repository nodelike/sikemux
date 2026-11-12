//! Where a cloud sign-in link opens, and the shortcut to the desktop it is on.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, ParentElement, Styled, Subscription,
    Window,
};
use sikemux_view::prefs::PreferenceChange;

use crate::settings::index::Page::SignOn;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(SignOn, "Single sign-on", "sso aws gcp login cloud"),
    row(
        SignOn,
        "Single sign-on",
        "Browser app",
        "chrome safari firefox arc",
    ),
    row(
        SignOn,
        "Single sign-on",
        "Workspace shortcut",
        "desktop space mission control",
    ),
];

/// The two fields, each setting its preference as it is typed in.
pub struct Fields {
    browser: Entity<InputState>,
    shortcut: Entity<InputState>,
    _typing: [Subscription; 2],
}

fn field(
    change: fn(String) -> PreferenceChange,
    value: String,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    });
    let typing = cx.subscribe(&input, move |bench, input, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            crate::settings::set(bench, change(input.read(cx).value().to_string()));
        }
    });
    (input, typing)
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let preferences = bench.preferences();
    let (browser, typed_browser) = field(
        PreferenceChange::CloudBrowser,
        preferences.cloud_browser.clone(),
        t!("Zen, Arc, Safari · empty = system default"),
        window,
        cx,
    );
    let (shortcut, typed_shortcut) = field(
        PreferenceChange::CloudBrowserShortcut,
        preferences.cloud_browser_shortcut.clone(),
        t!("ctrl+3 · empty = no switch"),
        window,
        cx,
    );
    let fields = Fields {
        browser,
        shortcut,
        _typing: [typed_browser, typed_shortcut],
    };
    super::change_pages(bench, cx, |pages, _| pages.cloud = Some(fields));
}

pub fn render(
    _: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    _: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.cloud else {
        return Vec::new();
    };
    let wide = |input: &Entity<InputState>| div().w(px(280.)).child(ui::Field::new(input));
    let body = div()
        .child(rows::row(
            t!("Browser app"),
            Some(t!(
                "Must match a running app’s name. A trailing .app is fine."
            )),
            wide(&fields.browser),
            cx,
        ))
        .child(rows::row(
            t!("Workspace shortcut"),
            Some(t!(
                "Fired right after the link opens, to reach the desktop the browser lives on."
            )),
            wide(&fields.shortcut),
            cx,
        ));
    vec![rows::section(
        n!("Single sign-on"),
        None,
        Some(t!("Where an AWS or GCP sign-in URL opens, and where to go once it does.").into()),
        lit("Single sign-on"),
        body,
        cx,
    )]
}
