//! Settings take the stage's place while the rails stay: one page at a time,
//! chosen from the list on their left or found by searching. Every change
//! goes to the core as it is made.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement,
    IntoElement, KeyBinding, ParentElement, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window,
};
use serde_json::{json, Value};

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

mod index;
mod pages;
pub(crate) use pages::session_sent;
pub(crate) mod rows;

pub use index::Page;

/// Named by today's command id, so a shortcut the person set for it still finds it.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "settings.toggle")]
pub struct ToggleSettings;

actions!(
    settings,
    [CloseSettings, FocusSearch, ResultUp, ResultDown, OpenResult]
);

/// What every project's agents did, as the Activity page watches it.
pub const ACTIVITY: &str = "activity-everywhere";

const CONTEXT: &str = "Settings";
const SEARCH: &str = "SettingsSearch";
/// How long a place found by searching stays lit.
const FLASH: Duration = Duration::from_millis(1600);

pub fn install(cx: &mut App) {
    let find = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    let search = Some("SettingsSearch > Input");
    cx.bind_keys([
        KeyBinding::new("escape", CloseSettings, Some(CONTEXT)),
        KeyBinding::new(find, FocusSearch, Some(CONTEXT)),
        KeyBinding::new("up", ResultUp, search),
        KeyBinding::new("down", ResultDown, search),
        KeyBinding::new("enter", OpenResult, search),
    ]);
}

/// Settings while they are open: the page shown, the search, and what each
/// page keeps between frames.
pub struct Settings {
    pub page: Page,
    search: Entity<InputState>,
    query: SharedString,
    chosen: usize,
    /// A place a search led to, lit for a moment.
    flash: Option<(&'static str, Instant)>,
    scroll: ScrollHandle,
    pages: pages::State,
    _typing: Subscription,
}

/// Opens settings on `page`, or shows `page` when they are open.
pub fn open(bench: &mut Workbench, page: Page, window: &mut Window, cx: &mut Context<Workbench>) {
    if let Some(settings) = bench.settings.as_mut() {
        settings.page = page;
        settings.query = SharedString::default();
        bench.parts.redraw(&bench.parts.stage, cx);
        return;
    }
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
    let typing = cx.subscribe(&search, |bench, search, event: &InputEvent, cx| {
        if let (InputEvent::Change, Some(settings)) = (event, bench.settings.as_mut()) {
            settings.query = search.read(cx).value();
            settings.chosen = 0;
            bench.parts.redraw(&bench.parts.stage, cx);
        }
    });
    search.update(cx, |search, cx| search.focus(window, cx));
    bench.settings = Some(Settings {
        page,
        search,
        query: SharedString::default(),
        chosen: 0,
        flash: None,
        scroll: ScrollHandle::new(),
        pages: pages::State::default(),
        _typing: typing,
    });
    cx.notify();
}

pub fn toggle(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    match bench.settings.is_some() {
        true => close(bench, window, cx),
        false => open(bench, Page::default(), window, cx),
    }
}

pub fn close(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let editing = bench
        .settings
        .as_ref()
        .and_then(|settings| settings.pages.appearance.as_ref())
        .is_some_and(|appearance| appearance.editor.is_some());
    if editing {
        pages::theme_editor::wear_picked(bench);
    }
    bench.settings = None;
    bench.focus.focus(window, cx);
    cx.notify();
}

/// What a shortcut does first while Settings covers the window, as on the Mac: most close
/// it before they run, and closing a tab or project closes only Settings.
pub fn before_shortcut(
    bench: &mut Workbench,
    keystroke: &gpui_kit::Keystroke,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if bench.settings.is_none() {
        return;
    }
    let Some(id) = crate::keymap::command_for(keystroke, cx) else {
        return;
    };
    let (keeps, only_closes) = crate::keymap::over_settings(&id);
    if !keeps {
        close(bench, window, cx);
    }
    if only_closes {
        cx.stop_propagation();
    }
}

pub use pages::remote_moved;

/// Sets one of the person's preferences in the core.
pub(crate) fn set(bench: &Workbench, key: &str, value: impl Into<Value>) {
    let _ = bench.asks.send(Ask::Call {
        method: "setPreference",
        params: json!({ "key": key, "value": value.into() }),
    });
}

/// Opens settings on `page` with `section` brought into view and lit, as a search hit is.
pub fn open_at(
    bench: &mut Workbench,
    page: Page,
    section: &'static str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    open(bench, page, window, cx);
    let entry = index::Entry {
        page,
        section,
        label: section,
        keywords: "",
        shortcut: None,
    };
    go_to(bench, entry, window, cx);
}

/// Takes the person to the place a search found, and lights it.
fn go_to(
    bench: &mut Workbench,
    entry: index::Entry,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(settings) = bench.settings.as_mut() else {
        return;
    };
    settings.page = entry.page;
    settings.query = SharedString::default();
    if entry.shortcut.is_some() {
        settings.pages.shortcut = entry.shortcut;
        settings.pages.keybindings = None;
    }
    settings.flash = Some((entry.section, Instant::now()));
    settings
        .search
        .update(cx, |search, cx| search.set_value("", window, cx));
    let section = pages::sections(entry.page)
        .iter()
        .position(|section| *section == entry.section);
    if let Some(at) = section {
        settings.scroll.scroll_to_item(at);
    }
    cx.spawn(async move |bench, cx| {
        cx.background_executor().timer(FLASH).await;
        let _ = bench.update(cx, |bench, cx| {
            if let Some(settings) = bench.settings.as_mut() {
                settings.flash = None;
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// Settings in the stage's place.
pub fn view(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(settings) = bench.settings.as_ref() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let found = index::search(&settings.query, cx);
    let searching = !settings.query.trim().is_empty();
    let close_keys = crate::keymap::hint("settings.toggle", cx)
        .map_or_else(|| "Esc".to_owned(), |keys| format!("Esc / {keys}"));

    let nav = index::GROUPS.iter().map(|(group, pages)| {
        div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                div()
                    .px(px(10.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_size(crate::theme::ui_text(10.5))
                    .text_color(t.text_tertiary)
                    .child(*group),
            )
            .children(pages.iter().map(|page| {
                let (page, this) = (*page, this.clone());
                let chosen = settings.page == page && !searching;
                div()
                    .id(SharedString::from(format!("settings-page-{}", page.name())))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .text_color(if chosen {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .when(chosen, |row| row.bg(t.surface_chosen))
                    .when(!chosen, |row| row.hover(|style| style.bg(t.surface_hover)))
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| open(bench, page, window, cx))
                    })
                    .child(ui::icon(page.icon(), 14.).text_color(t.text_tertiary))
                    .child(page.name())
            }))
    });

    let rail = div()
        .w(px(220.))
        .flex_none()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.border)
        .p(px(8.))
        .child(
            div()
                .key_context(SEARCH)
                .on_action(window.listener_for(&this, |bench, _: &ResultUp, _, cx| {
                    if let Some(settings) = bench.settings.as_mut() {
                        settings.chosen = settings.chosen.saturating_sub(1);
                        bench.parts.redraw(&bench.parts.stage, cx);
                    }
                }))
                .on_action(window.listener_for(&this, |bench, _: &ResultDown, _, cx| {
                    if let Some(settings) = bench.settings.as_mut() {
                        let found = index::search(&settings.query, cx).len();
                        settings.chosen = (settings.chosen + 1).min(found.saturating_sub(1));
                        bench.parts.redraw(&bench.parts.stage, cx);
                    }
                }))
                .on_action(
                    window.listener_for(&this, |bench, _: &OpenResult, window, cx| {
                        let chosen = bench.settings.as_ref().and_then(|settings| {
                            index::search(&settings.query, cx)
                                .get(settings.chosen)
                                .copied()
                        });
                        if let Some(entry) = chosen {
                            go_to(bench, entry, window, cx);
                        }
                    }),
                )
                .child(ui::Field::new(&settings.search).icon(Icon::Search).hint(
                    crate::keymap::label(if cfg!(target_os = "macos") {
                        "Meta+F"
                    } else {
                        "Ctrl+F"
                    }),
                )),
        )
        .child(div().flex_1().min_h_0().children(nav))
        .child(
            div()
                .px(px(10.))
                .py(px(8.))
                .text_size(crate::theme::ui_text(11.))
                .text_color(t.text_tertiary)
                .child("Changes save automatically"),
        );

    let title: SharedString = match searching {
        true => "Search".into(),
        false => settings.page.name().into(),
    };
    let topbar = div()
        .flex()
        .items_center()
        .h(px(44.))
        .px(px(20.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex_1()
                .text_size(crate::theme::ui_text(14.))
                .text_color(t.text_primary)
                .child(title),
        )
        .child({
            let this = this.clone();
            ui::IconButton::new(
                "settings-close",
                Icon::Close,
                format!("Close settings ({close_keys})"),
            )
            .on_click(move |_, window, cx| this.update(cx, |bench, cx| close(bench, window, cx)))
        });

    let body: Vec<AnyElement> = match searching {
        true if found.is_empty() => vec![div()
            .p(px(24.))
            .text_color(t.text_tertiary)
            .child(format!("No settings match “{}”", settings.query.trim()))
            .into_any_element()],
        true => found
            .iter()
            .enumerate()
            .map(|(at, entry)| {
                let (entry, this) = (*entry, this.clone());
                let chosen = at == settings.chosen;
                div()
                    .id(("settings-result", at))
                    .flex()
                    .flex_col()
                    .px(px(14.))
                    .py(px(8.))
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .when(chosen, |row| row.bg(t.surface_chosen))
                    .hover(|style| style.bg(t.surface_hover))
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| go_to(bench, entry, window, cx))
                    })
                    .child(div().text_color(t.text_primary).child(entry.label))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(11.5))
                            .text_color(t.text_tertiary)
                            .child(format!("{} › {}", entry.page.name(), entry.section)),
                    )
                    .into_any_element()
            })
            .collect(),
        false => {
            let flash = settings
                .flash
                .filter(|(_, at)| at.elapsed() < FLASH)
                .map(|(section, _)| section);
            pages::render(bench, settings, flash, &this, window, cx)
        }
    };

    div()
        .key_context(CONTEXT)
        .on_action(
            window.listener_for(&this, |bench, _: &CloseSettings, window, cx| {
                close(bench, window, cx)
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &FocusSearch, window, cx| {
                if let Some(settings) = &bench.settings {
                    settings
                        .search
                        .update(cx, |search, cx| search.focus(window, cx));
                }
            }),
        )
        .flex_1()
        .min_w_0()
        .child(
            ui::pane(cx).child(
                div().size_full().flex().child(rail).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(topbar)
                        .child(
                            div()
                                .id("settings-scroll")
                                .track_scroll(&settings.scroll)
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .px(px(28.))
                                .py(px(20.))
                                .flex()
                                .flex_col()
                                .gap(px(28.))
                                .children(body.into_iter().map(|part| {
                                    div().w_full().max_w(px(760.)).mx_auto().child(part)
                                })),
                        ),
                ),
            ),
        )
        .into_any_element()
}
