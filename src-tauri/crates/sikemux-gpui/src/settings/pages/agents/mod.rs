//! The agents Sikemux runs: which are installed, the accounts and profiles they
//! start on, the models they may use, their skills, and how sessions behave.

use gpui_kit::{AnyElement, App, Context, Entity, Window};

use crate::settings::Page;
use crate::settings::Settings;
use crate::workbench::Workbench;

mod accounts;
mod device;
mod profiles;
mod providers;
mod sessions;
mod setup;
mod skills;

/// What the page keeps between frames, made the first time it shows.
pub struct Fields {
    sessions: sessions::Fields,
    device: device::Fields,
    profiles: profiles::Fields,
    providers: providers::Fields,
    skills: skills::Fields,
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    crate::accounts::read(bench, cx);
    let fields = Fields {
        sessions: sessions::Fields::new(bench, window, cx),
        device: device::Fields::new(bench, window, cx),
        profiles: profiles::Fields::new(bench, window, cx),
        providers: providers::Fields::new(bench, window, cx),
        skills: skills::Fields::new(bench, window, cx),
    };
    super::change_pages(bench, cx, |pages, _| pages.agents = Some(fields));
    crate::agents::setup::read(bench, cx);
    sessions::read_names(bench, window, cx);
    device::read_simulator(bench, cx);
    providers::read(bench, cx);
    skills::read(bench, cx);
}

pub fn read_providers(bench: &Workbench, cx: &mut Context<Workbench>) {
    providers::read(bench, cx);
}

/// Changes the agent pages' forms, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.agents.as_mut(), change)
}

/// What the agent pages keep, while one is open.
fn fields<'a>(bench: &Workbench, cx: &'a App) -> Option<&'a Fields> {
    super::state(bench, cx)?.agents.as_ref()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    page: Page,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.agents else {
        return Vec::new();
    };
    match page {
        Page::Notifications => vec![sessions::notifications(bench, lit, this, cx)],
        Page::Installed => vec![setup::render(bench, &bench.agent_setup, lit, this, cx)],
        Page::Permissions => vec![sessions::boundary(bench, lit, this, cx)],
        Page::Chats => vec![sessions::render(&fields.sessions, lit, this, cx)],
        Page::Skills => vec![skills::render(&fields.skills, lit, this, cx)],
        Page::Accounts => {
            let mut sections: Vec<AnyElement> = signs_in(bench)
                .into_iter()
                .map(|provider| accounts::render(bench, &provider, lit, this, cx))
                .collect();
            sections.push(profiles::render(bench, &fields.profiles, lit, this, cx));
            sections
        }
        Page::Plans | Page::Providers | Page::LocalModels | Page::Helpers => {
            providers::render(bench, &fields.providers, page, lit, this, cx)
        }
        Page::Notch | Page::Voice | Page::Simulator => {
            device::render(bench, &fields.device, page, lit, this, cx)
        }
        _ => Vec::new(),
    }
}

/// Whether the helper's dropdowns on `page` were made for another helper than the one chosen.
pub(super) fn stale(fields: &Fields, page: Page) -> bool {
    matches!(
        page,
        Page::Plans | Page::Providers | Page::LocalModels | Page::Helpers
    ) && providers::stale(&fields.providers)
}

pub(super) use providers::rebuild_choices;

/// The agents the catalog says keep each sign-in apart, so the person can hold several accounts.
fn signs_in(bench: &Workbench) -> Vec<String> {
    bench
        .reads
        .catalog
        .agents()
        .filter(|kind| kind.accounts)
        .map(|kind| kind.kind.clone())
        .collect()
}

/// The add-account dialog and the install or sign-in sheet, over whichever page is open.
pub fn dialogs(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let mut open = Vec::new();
    open.extend(accounts::dialog(bench, this, cx));
    open.extend(crate::agents::setup::sheet(bench, this, window, cx));
    open
}
