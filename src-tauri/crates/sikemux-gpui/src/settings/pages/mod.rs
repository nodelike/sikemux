//! Each settings page, and what the pages keep between frames.

use gpui_kit::component::input::InputEvent;
use gpui_kit::{AnyElement, App, Context, Entity, EventEmitter, Subscription, Window};
use serde_json::Value;

pub use appearance::install;

use super::{Open, Page, Settings};
use crate::workbench::Workbench;

mod about;
mod actions;
mod activity;
mod agents;
mod appearance;
mod cli;
mod cloud;
mod devices;
mod general;
pub(crate) use general::sent as session_sent;
mod keybindings;
mod plugins;
pub mod theme_editor;

/// What the pages keep while settings are open: what the core last answered
/// for them, and the fields being typed in.
#[derive(Default)]
pub struct State {
    pub agents: Option<agents::Fields>,
    pub devices: Option<devices::Fields>,
    pub actions: Option<actions::Fields>,
    pub activity: Option<activity::Fields>,
    pub appearance: Option<appearance::Fields>,
    pub cloud: Option<cloud::Fields>,
    pub general: Option<general::Fields>,
    pub keybindings: Option<keybindings::Fields>,
    /// The command a search found, which the shortcuts page opens filtered to.
    pub shortcut: Option<&'static str>,
    /// The command line's install, as the core last said.
    pub cli: Option<Value>,
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

/// The sections of a page in the order it shows them, as searches name them.
pub fn sections(page: Page) -> Vec<&'static str> {
    super::index::sections(page)
}

/// Makes what the page shown keeps between frames, when it is about to be drawn without it.
pub fn prepare(bench: &Workbench, page: Page, window: &Window, cx: &mut Context<Workbench>) {
    let Some(pages) = state(bench, cx) else {
        return;
    };
    match page {
        Page::Account | Page::Devices | Page::Hosts if pages.devices.is_none() => {
            cx.defer_in(window, devices::make)
        }
        Page::General if pages.general.is_none() => cx.defer_in(window, general::make),
        Page::Keyboard if pages.keybindings.is_none() => cx.defer_in(window, keybindings::make),
        Page::Theme | Page::Interface | Page::Buddies if pages.appearance.is_none() => {
            cx.defer_in(window, appearance::make)
        }
        Page::SignOn if pages.cloud.is_none() => cx.defer_in(window, cloud::make),
        Page::Actions if pages.actions.is_none() => cx.defer_in(window, actions::make),
        Page::Cli if pages.cli.is_none() => {
            cx.defer_in(window, |bench, _, cx| cli::ask(bench, "cliInstall", cx))
        }
        Page::Usage if pages.activity.is_none() => cx.defer_in(window, activity::make),
        Page::Notifications
        | Page::Installed
        | Page::Permissions
        | Page::Chats
        | Page::Skills
        | Page::Helpers
        | Page::Accounts
        | Page::Plans
        | Page::Providers
        | Page::LocalModels
        | Page::Notch
        | Page::Voice
        | Page::Simulator => match &pages.agents {
            None => cx.defer_in(window, agents::make),
            Some(fields) if agents::stale(fields, page) => {
                cx.defer_in(window, agents::rebuild_choices)
            }
            Some(_) => {}
        },
        _ => {}
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
        Page::Account | Page::Devices | Page::Hosts => {
            devices::render(bench, pages, page, &lit, this, cx)
        }
        Page::General => general::render(bench, pages, &lit, this, cx),
        Page::Keyboard => keybindings::render(bench, pages, &lit, this, cx),
        Page::Theme | Page::Interface | Page::Buddies => {
            appearance::render(bench, pages, page, &lit, this, window, cx)
        }
        Page::Notifications
        | Page::Installed
        | Page::Permissions
        | Page::Chats
        | Page::Skills
        | Page::Helpers
        | Page::Accounts
        | Page::Plans
        | Page::Providers
        | Page::LocalModels
        | Page::Notch
        | Page::Voice
        | Page::Simulator => agents::render(bench, pages, page, &lit, this, cx),
        Page::Integrations => plugins::render(bench, &lit, this, cx),
        Page::SignOn => cloud::render(bench, pages, &lit, this, cx),
        Page::Actions => actions::render(bench, pages, &lit, this, cx),
        Page::Cli => cli::render(bench, pages, &lit, this, cx),
        Page::Usage => activity::render(bench, pages, &lit, this, cx),
        Page::About => about::render(bench, &lit, this, cx),
    }
}

/// The add-account dialog and the install or sign-in sheet, over whichever page is open.
pub fn dialogs(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    agents::dialogs(bench, this, window, cx)
}

/// Lists the model providers again while a page shows them, as after one is added.
pub fn providers_changed(bench: &Workbench, cx: &mut Context<Workbench>) {
    let shown = state(bench, cx).is_some_and(|pages| pages.agents.is_some());
    if shown {
        agents::read_providers(bench, cx);
    }
}

/// Lists the account's other hosts again while the Devices page shows them,
/// since a change to remote access may have linked or dropped one.
pub fn remote_moved(bench: &Workbench, cx: &mut Context<Workbench>) {
    let shown = state(bench, cx).is_some_and(|pages| pages.devices.is_some());
    if shown {
        devices::read_links(bench, cx);
    }
}
