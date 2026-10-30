//! Settings take the stage's place while the rails stay: one page at a time,
//! chosen from the list on their left or found by searching. Every change
//! goes to the core as it is made.

use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, div, px, AnyElement, App, AppContext, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, ParentElement,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use serde_json::{json, Value};

use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, SPACE_6, TEXT_2XS_H,
    TEXT_3XS, TEXT_XL, TEXT_XS_H,
};
use crate::ui;
use crate::workbench::{Strip, Workbench};

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
    pages::install(cx);
}

/// Settings, which stand in the stage's place while they are open.
#[derive(Default)]
pub struct Settings {
    open: Option<Open>,
}

impl Settings {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

/// Changes Settings while they are open, and draws them again.
fn change<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Open, &mut Context<Settings>) -> R,
) -> Option<R> {
    bench.settings.update(cx, |settings, cx| {
        let open = settings.open.as_mut()?;
        let changed = change(open, cx);
        cx.notify();
        Some(changed)
    })
}

/// Settings while they are open: the page shown, the search, and what each
/// page keeps between frames.
pub struct Open {
    pub page: Page,
    search: Entity<InputState>,
    query: SharedString,
    chosen: usize,
    /// A place a search led to, lit for a moment.
    flash: Option<(&'static str, Instant)>,
    scroll: ScrollHandle,
    /// Held while settings are open, so Escape and their other keys reach them wherever the
    /// person clicked.
    focus: FocusHandle,
    /// The list of pages on the left, which follows the page shown.
    nav: ScrollHandle,
    pages: pages::State,
    /// What was in front behind Settings when they opened.
    behind: Behind,
    _typing: Subscription,
}

/// Whose core, which strip and which of its tabs is in front.
type Behind = (crate::hosts::Place, Option<Strip>, Option<String>);

fn behind(bench: &Workbench) -> Behind {
    let tab = bench.tab().map(|tab| tab.id.clone());
    (bench.place(), bench.strip(), tab)
}

/// Opens settings on `page`, or shows `page` when they are open.
pub fn open(bench: &mut Workbench, page: Page, window: &mut Window, cx: &mut Context<Workbench>) {
    let shown = change(bench, cx, |settings, _| {
        settings.page = page;
        settings.query = SharedString::default();
        reveal(settings);
    });
    if shown.is_some() {
        return;
    }
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
    let typing = cx.subscribe(&search, |bench, search, event: &InputEvent, cx| {
        if let InputEvent::Change = event {
            let query = search.read(cx).value();
            change(bench, cx, |settings, _| {
                settings.query = query;
                settings.chosen = 0;
            });
        }
    });
    search.update(cx, |search, cx| search.focus(window, cx));
    let open = Open {
        page,
        search,
        query: SharedString::default(),
        chosen: 0,
        flash: None,
        scroll: ScrollHandle::new(),
        focus: cx.focus_handle(),
        nav: ScrollHandle::new(),
        pages: pages::State::default(),
        behind: behind(bench),
        _typing: typing,
    };
    reveal(&open);
    bench
        .settings
        .update(cx, |settings, _| settings.open = Some(open));
    // The list learns where its groups sit only once it is drawn, so it scrolls on the frame after.
    let settings = bench.settings.clone();
    cx.on_next_frame(window, move |_, _, cx| {
        settings.update(cx, |_, cx| cx.notify())
    });
    cx.notify();
}

/// Scrolls the list of pages so the page shown is in it: to its end for a page in the
/// lower half of the list, and to its start for the rest.
fn reveal(settings: &Open) {
    let all: Vec<Page> = Page::all().collect();
    let at = all
        .iter()
        .position(|page| *page == settings.page)
        .unwrap_or(0);
    match at * 2 > all.len() {
        true => settings.nav.scroll_to_bottom(),
        false => settings.nav.set_offset(gpui_kit::point(px(0.), px(0.))),
    }
}

pub fn toggle(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    match bench.settings.read(cx).is_open() {
        true => close(bench, window, cx),
        false => open(bench, Page::default(), window, cx),
    }
}

pub fn close(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    leave(bench, cx);
    bench.keys.focus.focus(window, cx);
}

/// Closes Settings, leaving the keyboard to whatever the stage gives it next.
fn leave(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let editing = (bench.settings.read(cx).open.as_ref())
        .and_then(|settings| settings.pages.appearance.as_ref())
        .is_some_and(|appearance| appearance.editor.is_some());
    if editing {
        pages::theme_editor::wear_picked(bench);
    }
    bench
        .settings
        .update(cx, |settings, _| settings.open = None);
    cx.notify();
}

/// Settings stand in for the place in front, so they close once it changes behind them, by any
/// means.
pub fn front_moved(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let behind_now = behind(bench);
    if (bench.settings.read(cx).open.as_ref()).is_some_and(|settings| settings.behind != behind_now)
    {
        leave(bench, cx);
    }
}

/// Closes Settings on a press inside `element`, as rows of the rails, tabs and the Git chip take
/// the person somewhere even when it is already in front.
pub fn leaves<E: InteractiveElement>(element: E, cx: &mut Context<Workbench>) -> E {
    element.capture_any_mouse_down(cx.listener(|bench, event: &MouseDownEvent, window, cx| {
        if event.button == MouseButton::Left && bench.settings.read(cx).is_open() {
            close(bench, window, cx);
        }
    }))
}

/// What a shortcut does first while Settings covers the window, as on the Mac: most close
/// it before they run, and closing a tab or project closes only Settings.
pub fn before_shortcut(
    bench: &mut Workbench,
    keystroke: &gpui_kit::Keystroke,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if !bench.settings.read(cx).is_open() {
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

pub use pages::{providers_changed, remote_moved};

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
    let shown = change(bench, cx, |settings, cx| {
        settings.page = entry.page;
        settings.query = SharedString::default();
        reveal(settings);
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
            settings.scroll.scroll_to_item(at + 1);
        }
    });
    if shown.is_none() {
        return;
    }
    let settings = bench.settings.downgrade();
    cx.spawn(async move |_, cx| {
        cx.background_executor().timer(FLASH).await;
        let _ = settings.update(cx, |settings, cx| {
            if let Some(settings) = settings.open.as_mut() {
                settings.flash = None;
            }
            cx.notify();
        });
    })
    .detach();
}

/// Settings in the stage's place.
pub fn view(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(settings) = bench.settings.read(cx).open.as_ref() else {
        return div().into_any_element();
    };
    let this = cx.entity();
    let mut dialogs = Vec::new();
    if settings.query.trim().is_empty() {
        let page = settings.page;
        cx.set_global(rows::Shown(page.name()));
        pages::prepare(bench, page, window, cx);
        dialogs = pages::dialogs(bench, &this, window, cx);
    }
    let Some(settings) = bench.settings.read(cx).open.as_ref() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let found = index::search(&settings.query, cx);
    let searching = !settings.query.trim().is_empty();
    let close_keys = crate::keymap::hint("settings.toggle", cx)
        .map_or_else(|| "Esc".to_owned(), |keys| format!("Esc / {keys}"));

    let page_row = |page: Page| {
        let this = this.clone();
        let chosen = settings.page == page && !searching;
        div()
            .id(SharedString::from(format!("settings-page-{}", page.name())))
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(24.))
            .px(SPACE_2)
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_size(crate::theme::ui_text(TEXT_XS_H))
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
            .child(ui::icon(page.icon(), 14.).text_color(if chosen {
                t.text_secondary
            } else {
                t.text_tertiary
            }))
            .child(div().flex_1().min_w_0().truncate().child(page.name()))
    };
    let nav = index::GROUPS
        .iter()
        .filter(|(_, pages)| pages.iter().any(|page| page.here()))
        .enumerate()
        .map(|(at, (group, pages))| {
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .px(SPACE_2)
                        .pt(px(if at == 0 { 10. } else { 20. }))
                        .pb(px(5.))
                        .child(ui::caps(group, cx))
                        .child(div().flex_1().h(px(1.)).bg(t.border)),
                )
                .children(
                    pages
                        .iter()
                        .copied()
                        .filter(|page| page.here())
                        .map(page_row),
                )
        });

    let account = bench.view.as_ref().and_then(|view| view.account.as_deref());
    let me = {
        let this = this.clone();
        let chosen = settings.page == Page::Account && !searching;
        let (mark, name): (AnyElement, SharedString) = match account {
            Some(account) => (
                crate::rail::avatar(bench, account, 16., &t).into_any_element(),
                account
                    .name
                    .clone()
                    .or_else(|| account.email.clone())
                    .unwrap_or_else(|| "Signed in".into())
                    .into(),
            ),
            None => (
                ui::icon(Icon::User, 14.)
                    .text_color(if chosen {
                        t.text_secondary
                    } else {
                        t.text_tertiary
                    })
                    .into_any_element(),
                "Not signed in".into(),
            ),
        };
        div()
            .id("settings-page-Account")
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(24.))
            .px(SPACE_2)
            .rounded(theme::RADIUS_2)
            .cursor_pointer()
            .text_size(crate::theme::ui_text(TEXT_XS_H))
            .text_color(if chosen {
                t.text_primary
            } else {
                t.text_secondary
            })
            .when(chosen, |row| row.bg(t.surface_chosen))
            .when(!chosen, |row| row.hover(|style| style.bg(t.surface_hover)))
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| open(bench, Page::Account, window, cx))
            })
            .child(div().w(px(14.)).flex().justify_center().child(mark))
            .child(div().flex_1().min_w_0().truncate().child(name))
    };

    let release = bench
        .connection
        .version
        .split_once('-')
        .map_or(bench.connection.version.as_str(), |(release, _)| release)
        .to_owned();
    let foot = div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .pt(SPACE_1H)
        .mt(SPACE_1H)
        .border_t_1()
        .border_color(t.border)
        .child(me)
        .children(index::FOOT.map(|page| {
            page_row(page).when(page == Page::About && !release.is_empty(), |row| {
                row.child(ui::mono(format!("v{release}"), TEXT_3XS, cx))
            })
        }));

    let rail = div()
        .w(px(212.))
        .h_full()
        .min_h_0()
        .flex_none()
        .flex()
        .flex_col()
        .bg(rows::card_fill(cx).opacity(0.7))
        .border_r_1()
        .border_color(t.border)
        .child(
            div()
                .key_context(SEARCH)
                .on_action(window.listener_for(&this, |bench, _: &ResultUp, _, cx| {
                    change(bench, cx, |settings, _| {
                        settings.chosen = settings.chosen.saturating_sub(1)
                    });
                }))
                .on_action(window.listener_for(&this, |bench, _: &ResultDown, _, cx| {
                    change(bench, cx, |settings, cx| {
                        let found = index::search(&settings.query, cx).len();
                        settings.chosen = (settings.chosen + 1).min(found.saturating_sub(1));
                    });
                }))
                .on_action(
                    window.listener_for(&this, |bench, _: &OpenResult, window, cx| {
                        let chosen = bench.settings.read(cx).open.as_ref().and_then(|settings| {
                            index::search(&settings.query, cx)
                                .get(settings.chosen)
                                .copied()
                        });
                        if let Some(entry) = chosen {
                            go_to(bench, entry, window, cx);
                        }
                    }),
                )
                .child(
                    ui::Field::new(&settings.search)
                        .flush()
                        .small()
                        .icon(Icon::Search)
                        .hint(crate::keymap::label(if cfg!(target_os = "macos") {
                            "Meta+F"
                        } else {
                            "Ctrl+F"
                        })),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .px(SPACE_2)
                .pb(SPACE_2)
                .child(
                    div()
                        .id("settings-nav")
                        .track_scroll(&settings.nav)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(nav),
                )
                .child(foot),
        );

    let (title, about): (SharedString, SharedString) = match searching {
        true => (
            "Search".into(),
            match found.len() {
                1 => "1 place matches".into(),
                count => format!("{count} places match").into(),
            },
        ),
        false => (settings.page.name().into(), settings.page.about().into()),
    };
    let header = div().flex().items_start().gap(SPACE_3).pb(SPACE_0).child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(ui::text(title, TEXT_XL, FontWeight::SEMIBOLD).text_color(t.text_primary))
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_secondary)
                    .child(about),
            ),
    );
    let close_button = {
        let this = this.clone();
        ui::IconButton::new(
            "settings-close",
            Icon::Close,
            format!("Close settings ({close_keys})"),
        )
        .on_click(move |_, window, cx| this.update(cx, |bench, cx| close(bench, window, cx)))
    };

    let body: Vec<AnyElement> = match searching {
        true if found.is_empty() => vec![div()
            .py(SPACE_6)
            .text_color(t.text_secondary)
            .child(format!("No settings match “{}”", settings.query.trim()))
            .into_any_element()],
        true => vec![div()
            .flex()
            .flex_col()
            .p(SPACE_1)
            .bg(rows::card_fill(cx))
            .border_1()
            .border_color(t.border)
            .rounded(theme::RADIUS_3)
            .children(found.iter().enumerate().map(|(at, entry)| {
                let (entry, this) = (*entry, this.clone());
                let chosen = at == settings.chosen;
                div()
                    .id(("settings-result", at))
                    .flex()
                    .flex_col()
                    .px(SPACE_3)
                    .py(px(7.))
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .when(chosen, |row| row.bg(t.surface_chosen))
                    .when(!chosen, |row| row.hover(|style| style.bg(t.surface_hover)))
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| go_to(bench, entry, window, cx))
                    })
                    .child(div().text_color(t.text_primary).child(entry.label))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(format!("{} › {}", entry.page.name(), entry.section)),
                    )
            }))
            .into_any_element()],
        false => {
            let flash = settings
                .flash
                .filter(|(_, at)| at.elapsed() < FLASH)
                .map(|(section, _)| section);
            let mut sections = pages::render(bench, settings, flash, &this, window, cx);
            sections.extend(dialogs);
            sections
        }
    };

    div()
        .key_context(CONTEXT)
        .track_focus(&settings.focus)
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&this, |bench, _: &MouseDownEvent, window, cx| {
                if let Some(settings) = &bench.settings.read(cx).open {
                    if !settings.focus.contains_focused(window, cx) {
                        settings.focus.clone().focus(window, cx);
                    }
                }
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &CloseSettings, window, cx| {
                close(bench, window, cx)
            }),
        )
        .on_action(
            window.listener_for(&this, |bench, _: &FocusSearch, window, cx| {
                if let Some(settings) = &bench.settings.read(cx).open {
                    let search = settings.search.clone();
                    search.update(cx, |search, cx| search.focus(window, cx));
                }
            }),
        )
        .flex_1()
        .min_w_0()
        .child(
            ui::pane(cx).child(
                div().size_full().flex().child(rail).child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .id("settings-scroll")
                                .size_full()
                                .track_scroll(&settings.scroll)
                                .flex_1()
                                .min_w_0()
                                .overflow_y_scroll()
                                .px(px(32.))
                                .pt(px(26.))
                                .pb(px(40.))
                                .flex()
                                .flex_col()
                                .gap(SPACE_4)
                                .child(div().w_full().max_w(px(760.)).mx_auto().child(header))
                                .children(body.into_iter().map(|part| {
                                    div().w_full().max_w(px(760.)).mx_auto().child(part)
                                })),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(12.))
                                .right(px(12.))
                                .child(close_button),
                        ),
                ),
            ),
        )
        .into_any_element()
}
