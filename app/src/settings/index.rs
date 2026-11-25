//! Every page of settings, and every place in them a search can find.

use crate::i18n::tr;
use gpui_kit::App;
use sikemux_corekit::accounts;

use super::pages;
use crate::icons::Icon;
use crate::keymap;
use crate::workbench::Workbench;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    Account,
    #[default]
    General,
    Appearance,
    Keyboard,
    Agents,
    Skills,
    Models,
    Connectors,
    ThisMac,
    Usage,
    About,
}

use Page::*;

/// The pages in the list on the left, under their groups. Account, Usage and About
/// close it, outside any group.
pub const GROUPS: [(&str, &[Page]); 3] = [
    (n!("App"), &[General, Appearance, Keyboard]),
    (n!("Agents"), &[Agents, Skills, Models]),
    (n!("Reach"), &[Connectors, ThisMac]),
];

/// The pages below the groups, at the foot of the list.
pub const FOOT: [Page; 2] = [Usage, About];

/// Pages only the Mac has: the notch, dictation and the iOS Simulator. Other systems
/// neither show them nor find them.
const MAC_ONLY: &[Page] = &[ThisMac];

/// Rows only the Mac has, on pages every system has: its window effects.
const MAC_ONLY_ROWS: &[(Page, &str)] = &[
    (Appearance, "Window see-through"),
    (Appearance, "Blur behind"),
];

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
                n!("Projects, startup, notifications and your other hosts"),
                Icon::Settings,
            ),
            Appearance => (
                n!("Appearance"),
                n!("Theme, text, and what shows behind each pane"),
                Icon::Contrast,
            ),
            Keyboard => (
                n!("Keyboard"),
                n!("Every shortcut, and your own commands"),
                Icon::Keyboard,
            ),
            Agents => (
                n!("Agents"),
                n!("Every agent you can run, where it's installed, who it's signed in as, and how its chats run"),
                Icon::Agent,
            ),
            Skills => (
                n!("Skills"),
                n!("What every agent Sikemux starts can use"),
                Icon::Book,
            ),
            Models => (
                n!("Models"),
                n!("The plans, providers and local models agents run on"),
                Icon::Cube,
            ),
            Connectors => (
                n!("Connectors"),
                n!("The services, apps and panels your agents reach"),
                Icon::Plug,
            ),
            ThisMac => (
                n!("This Mac"),
                n!("The notch, dictation and simulators"),
                Icon::Laptop,
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
        all.get(next).copied().unwrap_or(self)
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
        pages::appearance::ENTRIES,
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
        pages::actions::ENTRIES,
        pages::cli::ENTRIES,
        pages::outside::ENTRIES,
        pages::activity::ENTRIES,
        pages::about::ENTRIES,
    ]
    .into_iter()
    .flatten()
}

/// Whether `entry` is on this system, and its page draws it for this account.
fn here(entry: &Entry, has_orgs: bool) -> bool {
    entry.page.here()
        && (has_orgs || (entry.page, entry.label) != (Account, "Orgs"))
        && (cfg!(target_os = "macos") || !MAC_ONLY_ROWS.contains(&(entry.page, entry.label)))
}

/// The places whose words hold every word of `query`, those whose label
/// starts with it first, then those whose label holds it. Every command is a
/// place too, found by its name, what it does or its keys.
pub fn search(query: &str, bench: &Workbench, cx: &App) -> Vec<Entry> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let account = bench.this_view().and_then(|view| view.account.as_deref());
    let has_orgs = accounts::shown_orgs(account).next().is_some();
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
        .filter(|entry| here(entry, has_orgs))
        .map(|entry| (*entry, String::new()))
        .chain(shortcuts)
        .enumerate()
        .filter(|(_, (entry, more))| {
            let haystack = format!(
                "{} {} {} {} {} {} {} {more}",
                entry.label,
                tr(entry.label),
                entry.section,
                tr(entry.section),
                entry.page.name(),
                tr(entry.page.name()),
                entry.keywords
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .map(|(order, (entry, _))| {
            let label = tr(entry.label).to_lowercase();
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
