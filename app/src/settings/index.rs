//! Every page of settings, and every place in them a search can find.

use gpui_kit::App;

use super::pages;
use crate::icons::Icon;
use crate::keymap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    Account,
    #[default]
    General,
    Keyboard,
    Notifications,
    Theme,
    Interface,
    Buddies,
    Installed,
    Permissions,
    Chats,
    Skills,
    Helpers,
    Accounts,
    Plans,
    Providers,
    LocalModels,
    Notch,
    Voice,
    Simulator,
    Devices,
    Hosts,
    Connectors,
    Mods,
    SignOn,
    Actions,
    Cli,
    Usage,
    About,
}

use Page::*;

/// The pages in the list on the left, under their groups. Account, Usage and About
/// close it, outside any group.
pub const GROUPS: [(&str, &[Page]); 6] = [
    ("General", &[General, Keyboard, Notifications]),
    ("Appearance", &[Theme, Interface, Buddies]),
    ("Agents", &[Installed, Permissions, Chats, Skills, Helpers]),
    ("Models", &[Accounts, Plans, Providers, LocalModels]),
    ("This Mac", MAC_ONLY),
    (
        "Connect",
        &[Devices, Hosts, Connectors, Mods, SignOn, Actions, Cli],
    ),
];

/// The pages below the groups, at the foot of the list.
pub const FOOT: [Page; 2] = [Usage, About];

/// Pages only the Mac has: the notch, dictation and the iOS Simulator. Other systems
/// neither show them nor find them.
const MAC_ONLY: &[Page] = &[Notch, Voice, Simulator];

/// Sections only the Mac has, on pages every system has: its window effects.
const MAC_ONLY_SECTIONS: &[(Page, &str)] = &[(Interface, "Window")];

impl Page {
    /// What the list calls it, one line under its name on what it holds, and its icon.
    fn info(self) -> (&'static str, &'static str, Icon) {
        match self {
            Account => (
                "Account",
                "Your Sikemux account, which links your phone and hosts",
                Icon::User,
            ),
            General => (
                "General",
                "Where projects come from and how a session starts",
                Icon::Settings,
            ),
            Keyboard => (
                "Keyboard",
                "Every shortcut, and the keys you gave it",
                Icon::Keyboard,
            ),
            Notifications => (
                "Notifications",
                "How Sikemux tells you an agent needs you or a goal moved",
                Icon::Inbox,
            ),
            Theme => (
                "Theme",
                "One palette for the chrome, the editor and the terminal",
                Icon::Contrast,
            ),
            Interface => (
                "Interface",
                "Text size, what shows behind each pane, and the window",
                Icon::Text,
            ),
            Buddies => (
                "Buddies",
                "Characters that stand in for agents' logos",
                Icon::Heart,
            ),
            Installed => ("Installed", "The agent CLIs on this host", Icon::Agent),
            Permissions => (
                "Permissions",
                "What agents may do before they ask",
                Icon::Shield,
            ),
            Chats => ("Chats", "How chats behave while agents work", Icon::Message),
            Skills => (
                "Skills",
                "What every agent Sikemux starts can use",
                Icon::Book,
            ),
            Helpers => (
                "Helpers",
                "The model agents start their helpers on",
                Icon::Agents,
            ),
            Accounts => ("Accounts", "Subscriptions agents sign in with", Icon::User),
            Plans => (
                "Coding plans",
                "Monthly plans made for coding agents",
                Icon::Gauge,
            ),
            Providers => (
                "API providers",
                "Hosted models paid by the token",
                Icon::Network,
            ),
            LocalModels => ("Local models", "Models served on this host", Icon::Cube),
            Notch => ("Notch", "The island over the MacBook notch", Icon::Laptop),
            Voice => ("Voice", "Dictation into agents and terminals", Icon::Mic),
            Simulator => (
                "iOS Simulator",
                "Letting agents drive the iOS Simulator",
                Icon::Tap,
            ),
            Devices => (
                "Devices",
                "Your phone, and reaching this host from it",
                Icon::Phone,
            ),
            Hosts => ("Hosts", "Other hosts on your account", Icon::Server),
            Connectors => (
                "Connectors",
                "The services your agents reach for you",
                Icon::Plug,
            ),
            Mods => ("Mods", "Panels you and your agents made", Icon::Dashboard),
            SignOn => (
                "Single sign-on",
                "Cloud sign-ins started from Sikemux",
                Icon::LogIn,
            ),
            Actions => (
                "Actions",
                "Your own commands, one keystroke away",
                Icon::Run,
            ),
            Cli => (
                "Command line",
                "The sikemux command in your shell",
                Icon::Terminal,
            ),
            Usage => (
                "Usage",
                "What your agents did, and what it cost",
                Icon::Activity,
            ),
            About => ("About", "Version, updates and privacy", Icon::InfoCircle),
        }
    }

    pub fn name(self) -> &'static str {
        self.info().0
    }

    pub fn about(self) -> &'static str {
        self.info().1
    }

    pub fn icon(self) -> Icon {
        self.info().2
    }

    /// Whether this system has the page.
    pub fn here(self) -> bool {
        cfg!(target_os = "macos") || !MAC_ONLY.contains(&self)
    }

    /// Every page on this system, in the order the list shows them.
    pub fn all() -> impl Iterator<Item = Page> {
        GROUPS
            .iter()
            .flat_map(|(_, pages)| pages.iter().copied())
            .chain(std::iter::once(Account))
            .chain(FOOT)
            .filter(|page| page.here())
    }

    /// The page called `name`, as the list on the left shows it.
    pub fn named(name: &str) -> Option<Page> {
        Page::all().find(|page| page.name().eq_ignore_ascii_case(name))
    }

    /// The page after or before this one in the list, wrapping around.
    pub fn step(self, forward: bool) -> Page {
        let all: Vec<Page> = Page::all().collect();
        let at = all.iter().position(|page| *page == self).unwrap_or(0);
        let next = match forward {
            true => (at + 1) % all.len(),
            false => (at + all.len() - 1) % all.len(),
        };
        all[next]
    }
}

/// One place in settings a search can take the person to: a section, or a
/// row in one.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub page: Page,
    pub section: &'static str,
    pub label: &'static str,
    pub keywords: &'static str,
    /// The command a search for its shortcut found, which the page filters to.
    pub shortcut: Option<&'static str>,
}

pub(super) const fn section(page: Page, title: &'static str, keywords: &'static str) -> Entry {
    Entry {
        page,
        section: title,
        label: title,
        keywords,
        shortcut: None,
    }
}

pub(super) const fn row(
    page: Page,
    section: &'static str,
    label: &'static str,
    keywords: &'static str,
) -> Entry {
    Entry {
        page,
        section,
        label,
        keywords,
        shortcut: None,
    }
}

/// Every place a search can find, each page's kept with the page, in the order they rank.
fn entries() -> impl Iterator<Item = &'static Entry> {
    [
        pages::devices::ACCOUNT,
        pages::general::ENTRIES,
        pages::keybindings::ENTRIES,
        pages::agents::sessions::NOTIFICATIONS,
        pages::appearance::THEME,
        pages::appearance::INTERFACE,
        pages::appearance::BUDDIES,
        pages::appearance::WINDOW,
        pages::agents::setup::ENTRIES,
        pages::agents::sessions::ENTRIES,
        pages::agents::skills::ENTRIES,
        pages::agents::providers::HELPERS,
        pages::agents::accounts::ENTRIES,
        pages::agents::providers::MODELS,
        pages::agents::device::ENTRIES,
        pages::devices::ENTRIES,
        pages::connectors::ENTRIES,
        pages::mods::ENTRIES,
        pages::cloud::ENTRIES,
        pages::actions::ENTRIES,
        pages::cli::ENTRIES,
        pages::activity::ENTRIES,
        pages::about::ENTRIES,
    ]
    .into_iter()
    .flatten()
}

/// Whether `section` of `page` is on this system.
pub fn here(page: Page, section: &str) -> bool {
    page.here() && (cfg!(target_os = "macos") || !MAC_ONLY_SECTIONS.contains(&(page, section)))
}

/// The places whose words hold every word of `query`, those whose label
/// starts with it first, then those whose label holds it. Every command is a
/// place too, found by its name, what it does or its keys.
pub fn search(query: &str, cx: &App) -> Vec<Entry> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let shortcuts = keymap::shortcuts().iter().map(|shortcut| {
        let keys = keymap::hint(shortcut.id, cx).unwrap_or_default();
        let entry = Entry {
            page: Keyboard,
            section: "Shortcuts",
            label: shortcut.label,
            keywords: shortcut.detail,
            shortcut: Some(shortcut.id),
        };
        (entry, format!("shortcut {keys}"))
    });
    let mut found: Vec<(usize, usize, Entry)> = entries()
        .filter(|entry| here(entry.page, entry.section))
        .map(|entry| (*entry, String::new()))
        .chain(shortcuts)
        .enumerate()
        .filter(|(_, (entry, more))| {
            let haystack = format!(
                "{} {} {} {} {more}",
                entry.label,
                entry.section,
                entry.page.name(),
                entry.keywords
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .map(|(order, (entry, _))| {
            let label = entry.label.to_lowercase();
            let rank = match () {
                _ if label.starts_with(&query) => 0,
                _ if label.contains(&query) => 1,
                _ => 2,
            };
            (rank, order, entry)
        })
        .collect();
    found.sort_by_key(|(rank, order, _)| (*rank, *order));
    found.into_iter().map(|(_, _, entry)| entry).collect()
}
