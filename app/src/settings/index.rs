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
    Settled,
    Skills,
    Helpers,
    Accounts,
    Plans,
    Providers,
    LocalModels,
    Notch,
    Voice,
    Simulator,
    Connectors,
    Mods,
    SignOn,
    Actions,
    Cli,
    OtherApps,
    Usage,
    About,
}

use Page::*;

/// The pages in the list on the left, under their groups. Account, Usage and About
/// close it, outside any group.
pub const GROUPS: [(&str, &[Page]); 6] = [
    (n!("General"), &[General, Keyboard, Notifications]),
    (n!("Appearance"), &[Theme, Interface, Buddies]),
    (
        n!("Agents"),
        &[Installed, Permissions, Chats, Settled, Skills, Helpers],
    ),
    (n!("Models"), &[Accounts, Plans, Providers, LocalModels]),
    (n!("This Mac"), MAC_ONLY),
    (n!("Connect"), &[Connectors, Mods, SignOn, Actions, Cli, OtherApps]),
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
                n!("Account"),
                n!("Your Sikemux account, which links your phone and hosts"),
                Icon::User,
            ),
            General => (
                n!("General"),
                n!("Where projects come from and how a session starts"),
                Icon::Settings,
            ),
            Keyboard => (
                n!("Keyboard"),
                n!("Every shortcut, and the keys you gave it"),
                Icon::Keyboard,
            ),
            Notifications => (
                n!("Notifications"),
                n!("How Sikemux tells you an agent needs you or a goal moved"),
                Icon::Inbox,
            ),
            Theme => (
                n!("Theme"),
                n!("One palette for the chrome, the editor and the terminal"),
                Icon::Contrast,
            ),
            Interface => (
                n!("Interface"),
                n!("Text size, what shows behind each pane, and the window"),
                Icon::Text,
            ),
            Buddies => (
                n!("Buddies"),
                n!("Characters that stand in for agents' logos"),
                Icon::Heart,
            ),
            Installed => (
                n!("Installed"),
                n!("The agent CLIs on this host"),
                Icon::Agent,
            ),
            Permissions => (
                n!("Permissions"),
                n!("What agents may do before they ask"),
                Icon::Shield,
            ),
            Chats => (
                n!("Chats"),
                n!("How chats behave while agents work"),
                Icon::Message,
            ),
            Settled => (
                n!("Settled chats"),
                n!("Chats out of the rail, to find and bring back"),
                Icon::Check,
            ),
            Skills => (
                n!("Skills"),
                n!("What every agent Sikemux starts can use"),
                Icon::Book,
            ),
            Helpers => (
                n!("Helpers"),
                n!("The model agents start their helpers on"),
                Icon::Agents,
            ),
            Accounts => (
                n!("Accounts"),
                n!("Subscriptions agents sign in with"),
                Icon::User,
            ),
            Plans => (
                n!("Coding plans"),
                n!("Monthly plans made for coding agents"),
                Icon::Gauge,
            ),
            Providers => (
                n!("API providers"),
                n!("Hosted models paid by the token"),
                Icon::Network,
            ),
            LocalModels => (
                n!("Local models"),
                n!("Models served on this host"),
                Icon::Cube,
            ),
            Notch => (
                n!("Notch"),
                n!("The island over the MacBook notch"),
                Icon::Laptop,
            ),
            Voice => (
                n!("Voice"),
                n!("Dictation into agents and terminals"),
                Icon::Mic,
            ),
            Simulator => (
                n!("Simulators"),
                n!("Letting agents drive iOS simulators and Android devices"),
                Icon::Tap,
            ),
            Connectors => (
                n!("Connectors"),
                n!("The services your agents reach for you"),
                Icon::Plug,
            ),
            Mods => (
                n!("Mods"),
                n!("Panels you and your agents made"),
                Icon::Dashboard,
            ),
            SignOn => (
                n!("Single sign-on"),
                n!("Cloud sign-ins started from Sikemux"),
                Icon::LogIn,
            ),
            Actions => (
                n!("Actions"),
                n!("Your own commands, one keystroke away"),
                Icon::Run,
            ),
            Cli => (
                n!("Command line"),
                n!("The sikemux command in your shell"),
                Icon::Terminal,
            ),
            OtherApps => (
                n!("Other apps"),
                n!("Claude Code, Codex and other MCP apps using your chats"),
                Icon::Plug,
            ),
            Usage => (
                n!("Usage"),
                n!("What your agents did, and what it cost"),
                Icon::Activity,
            ),
            About => (
                n!("About"),
                n!("Version, updates and privacy"),
                Icon::InfoCircle,
            ),
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
        pages::account::ENTRIES,
        pages::general::ENTRIES,
        pages::keybindings::ENTRIES,
        pages::agents::sessions::NOTIFICATIONS,
        pages::appearance::THEME,
        pages::appearance::INTERFACE,
        pages::appearance::BUDDIES,
        pages::appearance::WINDOW,
        pages::agents::setup::ENTRIES,
        pages::agents::sessions::ENTRIES,
        pages::settled::ENTRIES,
        pages::agents::skills::ENTRIES,
        pages::agents::providers::HELPERS,
        pages::agents::accounts::ENTRIES,
        pages::agents::providers::MODELS,
        pages::agents::device::ENTRIES,
        pages::connectors::ENTRIES,
        pages::mods::ENTRIES,
        pages::cloud::ENTRIES,
        pages::actions::ENTRIES,
        pages::cli::ENTRIES,
        pages::outside::ENTRIES,
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
        let keys = keymap::hint_named(shortcut.id, cx).unwrap_or_default();
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
