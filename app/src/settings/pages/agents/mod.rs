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
use crate::settings::index::Page::{
    Accounts, Chats, Helpers, Installed, LocalModels, Notch, Notifications, Permissions, Plans,
    Providers, Simulator, Skills, Voice,
};
use crate::settings::index::{row, section, Entry};

/// What a search finds on this page.
pub(in crate::settings) const NOTIFICATIONS: &[Entry] = &[
    section(
        Notifications,
        "Notifications",
        "notification alert badge dock permission finished",
    ),
    row(
        Notifications,
        "Notifications",
        "Notify when an agent needs you",
        "notification alert badge dock permission finished",
    ),
    row(
        Notifications,
        "Notifications",
        "Notify when a goal moves",
        "notification goal slice landed finished eta slipped progress",
    ),
    row(
        Notifications,
        "Notifications",
        "Test notification",
        "notification sound permission send try",
    ),
];

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(Installed, "Agents on this host", "install reinstall missing claude codex opencode hermes pi omp grok cli"),
    section(Permissions, "Launch boundary", "permissions yolo sandbox safety bypass approval"),
    section(Chats, "Chats", "sessions"),
    row(Chats, "Chats", "Sending while an agent works", "steer queue interrupt enter"),
    row(Chats, "Chats", "Rail density", "compact comfortable sidebar"),
    row(Chats, "Chats", "Agent names", "name theme anime nature naruto one piece characters words"),
    row(Chats, "Chats", "Idle chats", "sleep memory process agents"),
    section(Skills, "Skills", "skill SKILL.md library claude codex opencode pi omp hermes grok global own move"),
    row(Skills, "Skills", "Add a skill", "new create install git folder"),
    row(Skills, "Skills", "Your own skills", "global folder move adopt"),
    section(Helpers, "Helpers", "cheap model workers scouts reviewers subagents spawn paid by provider"),
    row(Helpers, "Helpers", "Agent", "helper worker scout reviewer subagent cheap model provider"),
    section(Accounts, "Accounts", "profiles executable path sign in login logout email subscription max pro plus chatgpt switch rotate limit add second work personal claude codex"),
    section(Plans, "Coding plans", "zai glm minimax kimi code opencode go synthetic ollama subscription plan key claude codex"),
    section(Providers, "API providers", "openrouter baseten together fireworks groq cerebras deepseek moonshot kimi mistral vercel api key hosted open models opencode pi omp claude codex"),
    section(LocalModels, "Local models", "ollama lm studio lmstudio local offline runner claude codex"),
    section(Notch, "Notch", "island dynamic menu bar notch overlay agents running"),
    row(Notch, "Notch", "Show the island", "notch dynamic island enable disable"),
    row(Notch, "Notch", "Show on", "display monitor external screen"),
    row(Notch, "Notch", "Open with", "hover click"),
    row(Notch, "Notch", "In full-screen apps", "fullscreen hide"),
    row(Notch, "Notch", "Peek when", "peek finished needs you"),
    row(Notch, "Notch", "Answer permissions in the notch", "allow deny permission"),
    row(Notch, "Notch", "Sound when an agent needs you", "sound alert"),
    row(Notch, "Notch", "Haptic feedback", "haptics trackpad force touch tick vibration swipe"),
    row(Notch, "Notch", "Step aside for Sikemux Dev", "dev build development"),
    section(Voice, "Voice", "dictation microphone speech talk transcribe"),
    row(Voice, "Voice", "Dictate with right Option", "dictation microphone speech push to talk hold"),
    row(Voice, "Voice", "Speech model", "parakeet download neural engine"),
    section(Simulator, "iOS Simulator", "iphone ipad xcode devicehub simulator phone mobile bezel frame buttons"),
    row(Simulator, "iOS Simulator", "Let agents drive the iOS Simulator", "iphone tap swipe app test mobile"),
    row(Simulator, "iOS Simulator", "Xcode", "xcode-select developer tools"),
    row(Simulator, "iOS Simulator", "iOS runtimes", "ios version runtime"),
    row(Simulator, "iOS Simulator", "Simulator helper", "download idb"),
];

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
