//! Where a cloud sign-in link opens, and the shortcut to the desktop it is on.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, ParentElement, Styled, Subscription, Window,
};

use crate::settings::rows;
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Single sign-on"];

/// The two fields, each setting its preference as it is typed in.
pub struct Fields {
    browser: Entity<InputState>,
    shortcut: Entity<InputState>,
    _typing: [Subscription; 2],
}

fn field(
    key: &'static str,
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
            crate::settings::set(bench, key, input.read(cx).value().to_string());
        }
    });
    (input, typing)
}

fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let preferences = bench.preferences();
    let (browser, typed_browser) = field(
        "cloudBrowser",
        preferences.cloud_browser,
        "Zen, Arc, Safari · empty = system default",
        window,
        cx,
    );
    let (shortcut, typed_shortcut) = field(
        "cloudBrowserShortcut",
        preferences.cloud_browser_shortcut,
        "ctrl+3 · empty = no switch",
        window,
        cx,
    );
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.cloud = Some(Fields {
            browser,
            shortcut,
            _typing: [typed_browser, typed_shortcut],
        });
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

pub fn render(
    _: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    _: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.cloud else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let wide = |input: &Entity<InputState>| div().w(px(280.)).child(ui::Field::new(input));
    let body = div()
        .child(rows::row(
            "Browser app",
            Some("Must match a running app’s name. A trailing .app is fine."),
            wide(&fields.browser),
            cx,
        ))
        .child(rows::row(
            "Workspace shortcut",
            Some("Fired right after the link opens, to reach the desktop the browser lives on."),
            wide(&fields.shortcut),
            cx,
        ));
    vec![rows::section(
        "Single sign-on",
        None,
        Some("Where an AWS or GCP sign-in URL opens, and where to go once it does.".into()),
        lit("Single sign-on"),
        body,
        cx,
    )]
}
