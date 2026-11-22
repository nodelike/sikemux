//! Each settings page, and what the pages keep between frames.

use gpui_kit::component::input::InputEvent;
use gpui_kit::{AnyElement, App, Context, Entity, EventEmitter, Subscription, Window};

pub use activity::week_read;
pub use appearance::folded;

use super::{Open, Page, Settings};
use crate::workbench::Workbench;

pub(super) mod about;
pub(super) mod account;
pub(super) mod actions;
pub(super) mod activity;
pub(super) mod agents;
pub(super) mod appearance;
pub(super) mod cli;
pub(super) mod connectors;
pub(super) mod general;
pub(crate) use general::sent as session_sent;
pub(super) mod keybindings;
pub(super) mod mods;
pub(super) mod outside;
pub(super) mod settled;
pub mod theme_editor;
mod vault;
mod week_card;

/// What the pages keep while settings are open: what the core last answered
/// for them, and the fields being typed in.
#[derive(Default)]
pub struct State {
    pub agents: Option<agents::Fields>,
    pub actions: Option<actions::Fields>,
    pub activity: Option<activity::Fields>,
    pub appearance: Option<appearance::Fields>,
    /// Whether Appearance shows its Advanced controls.
    pub advanced: bool,
    pub connectors: Option<connectors::Fields>,
    pub general: Option<general::Fields>,
    pub keybindings: Option<keybindings::Fields>,
    pub settled: Option<settled::Fields>,
    /// The command a search found, which the shortcuts page opens filtered to.
    pub shortcut: Option<&'static str>,
    /// The command line's install, once the page has asked, and none until the core says.
    pub cli: Option<Option<sikemux_calls::cli_install::CliInstallStatus>>,
    pub cli_busy: bool,
}

/// Changes what the pages keep while Settings are open, and draws them again.
fn change_pages<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut State, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change(bench, cx, |settings, cx| change(&mut settings.pages, cx))
}

/// Changes the form `form` finds while it is open, and draws the pages again.
fn change_form<F, R>(
    bench: &Workbench,
    cx: &mut App,
    form: impl FnOnce(&mut State) -> Option<&mut F>,
    change: impl FnOnce(&mut F, &mut Context<Settings>) -> R,
) -> Option<R> {
    change_pages(bench, cx, |pages, cx| Some(change(form(pages)?, cx))).flatten()
}

/// Draws the pages again as the person types in `input`.
fn follow_typing<T: EventEmitter<InputEvent>>(
    bench: &Workbench,
    input: &Entity<T>,
    cx: &mut App,
) -> Subscription {
    bench.settings.update(cx, |_, cx| {
        cx.subscribe(input, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
    })
}

/// What the pages keep, while Settings are open.
fn state<'a>(bench: &Workbench, cx: &'a App) -> Option<&'a State> {
    Some(&bench.settings.read(cx).open.as_ref()?.pages)
}

/// Makes what the page shown keeps between frames, when it is about to be drawn without it.
pub fn prepare(bench: &Workbench, page: Page, window: &Window, cx: &mut Context<Workbench>) {
    type Make = fn(&mut Workbench, &mut Window, &mut Context<Workbench>);
    let Some(pages) = state(bench, cx) else {
        return;
    };
    let agents: Option<Make> = match &pages.agents {
        None => Some(agents::make),
        Some(fields) if agents::stale(fields, page) => Some(agents::rebuild_choices),
        Some(_) => None,
    };
    let connectors: Option<Make> = match &pages.connectors {
        None => Some(connectors::make),
        Some(fields) if fields.stale => Some(connectors::read),
        Some(_) => None,
    };
    let missing = |made: bool, make: Make| (!made).then_some(make);
    let wanted: Vec<Make> = match page {
        Page::General => [missing(pages.general.is_some(), general::make), agents].into(),
        Page::Keyboard => [
            missing(pages.keybindings.is_some(), keybindings::make),
            missing(pages.actions.is_some(), actions::make),
        ]
        .into(),
        Page::Appearance => [missing(pages.appearance.is_some(), appearance::make)].into(),
        Page::Agents => [missing(pages.settled.is_some(), settled::make), agents].into(),
        Page::Connectors => [
            connectors,
            missing(pages.cli.is_some(), |bench, _, cx| cli::read(bench, cx)),
        ]
        .into(),
        Page::Usage => [missing(pages.activity.is_some(), activity::make)].into(),
        Page::Skills | Page::Models | Page::ThisMac => [agents].into(),
        Page::Account | Page::About => Vec::new(),
    }
    .into_iter()
    .flatten()
    .collect();
    for make in wanted {
        cx.defer_in(window, make);
    }
}

/// The sections of the page settings show.
pub fn render(
    bench: &Workbench,
    settings: &Open,
    flash: Option<&str>,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &App,
) -> Vec<AnyElement> {
    let lit = |section: &str| flash == Some(section);
    let pages = &settings.pages;
    let page = settings.page;
    match page {
        Page::Account => account::render(bench, &lit, this, cx),
        Page::General => {
            let mut sections = general::render(bench, pages, &lit, this, cx);
            sections.extend(agents::render(bench, pages, page, &lit, this, cx));
            sections
        }
        Page::Keyboard => {
            let mut sections = keybindings::render(bench, pages, &lit, this, cx);
            sections.extend(actions::render(bench, pages, &lit, this, cx));
            sections
        }
        Page::Appearance => appearance::render(bench, pages, &lit, this, window, cx),
        Page::Agents => {
            let mut sections = agents::render(bench, pages, page, &lit, this, cx);
            sections.extend(settled::render(bench, pages, &lit, this, cx));
            sections
        }
        Page::Skills | Page::Models | Page::ThisMac => {
            agents::render(bench, pages, page, &lit, this, cx)
        }
        Page::Connectors => {
            let mut sections = connectors::render(bench, pages, &lit, this, cx);
            sections.extend(outside::render(bench, &lit, this, cx));
            sections.extend(cli::render(bench, pages, &lit, this, cx));
            sections.extend(mods::render(bench, &lit, this, cx));
            sections
        }
        Page::Usage => activity::render(bench, pages, &lit, this, cx),
        Page::About => about::render(bench, &lit, this, cx),
    }
}

/// The add-account dialog, the install or sign-in sheet and a connector's sign-in, over
/// whichever page is open.
pub fn dialogs(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let mut open = agents::dialogs(bench, this, window, cx);
    open.extend(connectors::dialog(bench, this, cx));
    open
}

/// Lists the model providers again while a page shows them, as after one is added.
pub fn providers_changed(bench: &Workbench, cx: &mut Context<Workbench>) {
    let shown = state(bench, cx).is_some_and(|pages| pages.agents.is_some());
    if shown {
        agents::read_providers(bench, cx);
    }
}

/// Reads the devices again while a page shows them, as after the Android Emulator is set up.
pub fn devices_changed(bench: &Workbench, cx: &mut Context<Workbench>) {
    let shown = state(bench, cx).is_some_and(|pages| pages.agents.is_some());
    if shown {
        agents::read_devices(bench, cx);
    }
}

/// Reads the connectors' own settings again while the Connectors page shows them, as after a
/// sign-in, once it next draws.
pub fn plugins_changed(bench: &Workbench, cx: &mut App) {
    change_form(
        bench,
        cx,
        |pages| pages.connectors.as_mut(),
        |fields, _| fields.stale = true,
    );
}
