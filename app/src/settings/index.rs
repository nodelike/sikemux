//! Every page of settings, and every place in them a search can find.

use gpui_kit::App;

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
    Integrations,
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
    ("This Mac", &[Notch, Voice, Simulator]),
    (
        "Connect",
        &[Devices, Hosts, Integrations, SignOn, Actions, Cli],
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
    pub fn name(self) -> &'static str {
        match self {
            Account => "Account",
            General => "General",
            Keyboard => "Keyboard",
            Notifications => "Notifications",
            Theme => "Theme",
            Interface => "Interface",
            Buddies => "Buddies",
            Installed => "Installed",
            Permissions => "Permissions",
            Chats => "Chats",
            Skills => "Skills",
            Helpers => "Helpers",
            Accounts => "Accounts",
            Plans => "Coding plans",
            Providers => "API providers",
            LocalModels => "Local models",
            Notch => "Notch",
            Voice => "Voice",
            Simulator => "iOS Simulator",
            Devices => "Devices",
            Hosts => "Hosts",
            Integrations => "Integrations",
            SignOn => "Single sign-on",
            Actions => "Actions",
            Cli => "Command line",
            Usage => "Usage",
            About => "About",
        }
    }

    /// One line under the page's name on what it holds.
    pub fn about(self) -> &'static str {
        match self {
            Account => "Your Sikemux account, which links your phone and hosts",
            General => "Where projects come from and how a session starts",
            Keyboard => "Every shortcut, and the keys you gave it",
            Notifications => "How Sikemux tells you an agent needs you",
            Theme => "One palette for the chrome, the editor and the terminal",
            Interface => "Text size, what shows behind each pane, and the window",
            Buddies => "Characters that stand in for agents' logos",
            Installed => "The agent CLIs on this host",
            Permissions => "What agents may do before they ask",
            Chats => "How chats behave while agents work",
            Skills => "What every agent Sikemux starts can use",
            Helpers => "The model agents start their helpers on",
            Accounts => "Subscriptions agents sign in with",
            Plans => "Monthly plans made for coding agents",
            Providers => "Hosted models paid by the token",
            LocalModels => "Models served on this host",
            Notch => "The island over the MacBook notch",
            Voice => "Dictation into agents and terminals",
            Simulator => "Letting agents drive the iOS Simulator",
            Devices => "Your phone, and reaching this host from it",
            Hosts => "Other hosts on your account",
            Integrations => "The services built into Sikemux",
            SignOn => "Cloud sign-ins started from Sikemux",
            Actions => "Your own commands, one keystroke away",
            Cli => "The sikemux command in your shell",
            Usage => "What your agents did, and what it cost",
            About => "Version, updates and privacy",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Account => Icon::User,
            General => Icon::Settings,
            Keyboard => Icon::Keyboard,
            Notifications => Icon::Inbox,
            Theme => Icon::Contrast,
            Interface => Icon::Text,
            Buddies => Icon::Heart,
            Installed => Icon::Agent,
            Permissions => Icon::Shield,
            Chats => Icon::Message,
            Skills => Icon::Book,
            Helpers => Icon::Agents,
            Accounts => Icon::User,
            Plans => Icon::Gauge,
            Providers => Icon::Network,
            LocalModels => Icon::Cube,
            Notch => Icon::Laptop,
            Voice => Icon::Mic,
            Simulator => Icon::Tap,
            Devices => Icon::Phone,
            Hosts => Icon::Server,
            Integrations => Icon::Plug,
            SignOn => Icon::LogIn,
            Actions => Icon::Run,
            Cli => Icon::Terminal,
            Usage => Icon::Activity,
            About => Icon::InfoCircle,
        }
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

const fn section(page: Page, title: &'static str, keywords: &'static str) -> Entry {
    Entry {
        page,
        section: title,
        label: title,
        keywords,
        shortcut: None,
    }
}

const fn row(
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

pub const ENTRIES: &[Entry] = &[
    section(Account, "Account", "account sign in sign out login google github email delete"),
    section(General, "Project folders", "repos repositories directories roots scan depth index picker"),
    section(General, "Startup", ""),
    row(General, "Startup", "Restore agent tabs", "resume reopen startup"),
    section(General, "Session transfer", "send move copy host machine chats tabs another server transfer"),
    section(Keyboard, "Shortcuts", "keybindings hotkeys keys keyboard remap"),
    section(Notifications, "Notifications", "notification alert badge dock permission finished"),
    row(Notifications, "Notifications", "Notify when an agent needs you", "notification alert badge dock permission finished"),
    row(Notifications, "Notifications", "Test notification", "notification sound permission send try"),
    section(Theme, "Wearing", "colours colors palette dark light custom fork editor wallpaper customize"),
    section(Theme, "Themes", "colours colors palette dark light custom ghostty search"),
    section(Interface, "Text", ""),
    row(Interface, "Text", "Text size", "font zoom scale larger smaller accessibility"),
    section(Interface, "Panes", ""),
    row(Interface, "Panes", "Pane texture", "dither dithering shader grain noise background effect"),
    row(Interface, "Panes", "Pane style", "ascii art characters text letters glyphs dither dots matrix jellyfish"),
    row(Interface, "Panes", "Pane image", "picture photo wallpaper background dither dithered custom"),
    row(Interface, "Panes", "Pane strength", "dither grain picture opacity intensity texture image"),
    row(Interface, "Panes", "Colours", "pane picture dither inks palette theme accent custom gradient map"),
    row(Interface, "Panes", "Inks", "pane picture dither colours palette custom gradient map"),
    row(Interface, "Panes", "Pattern", "pane picture dither bayer ordered noise grid matrix"),
    row(Interface, "Panes", "Dot size", "pane picture dither pixel scale cell"),
    row(Interface, "Panes", "Contrast", "pane picture dither tone levels"),
    row(Interface, "Panes", "Brightness", "pane picture dither tone exposure midtones"),
    row(Interface, "Panes", "Glow", "pane picture dither bloom halation light"),
    row(Interface, "Panes", "Glow size", "pane picture dither bloom radius spread"),
    row(Interface, "Panes", "Covers", "pane picture dither band top whole full coverage"),
    section(Buddies, "Buddies", "agent icon character avatar mascot face"),
    row(Buddies, "Buddies", "Show buddies", "agent icon character avatar mascot harness logo"),
    row(Buddies, "Buddies", "Pack", "agent icon character avatar mascot style"),
    row(Buddies, "Buddies", "Shape", "agent icon character avatar mixed"),
    row(Buddies, "Buddies", "Animate faces", "motion blink animation"),
    section(Interface, "Window", ""),
    row(Interface, "Window", "Opacity", "transparency translucent see-through"),
    row(Interface, "Window", "Background blur", "vibrancy frosted glass"),
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
    section(Accounts, "Accounts", "sign in login logout email subscription max pro plus chatgpt switch rotate limit add second work personal claude codex"),
    row(Accounts, "Accounts", "Profiles", "agent profiles claude codex executable path profile directory config home"),
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
    section(Devices, "Devices", "phone mobile iphone android remote away connect host enable turn on off"),
    section(Devices, "Phones", "phone mobile iphone android qr code scan revoke remove forget access watch full control"),
    section(Hosts, "Hosts", "linked hosts machines servers rename remove account link"),
    section(Integrations, "Integrations", "aws bitbucket bruno database postgres postgresql mysql sqlite sql github actions gitlab jira rundeck signoz slack enable disable switch off turn on extensions integrations plugins"),
    section(SignOn, "Single sign-on", "sso aws gcp login cloud"),
    row(SignOn, "Single sign-on", "Browser app", "chrome safari firefox arc"),
    row(SignOn, "Single sign-on", "Workspace shortcut", "desktop space mission control"),
    section(Actions, "Your actions", "custom commands command deck scripts"),
    section(Actions, "New action", "custom command add create"),
    row(Actions, "New action", "Name", "title"),
    row(Actions, "New action", "Description", "detail"),
    row(Actions, "New action", "Command", "shell script"),
    row(Actions, "New action", "Where output lands", "placement terminal split popup background"),
    row(Actions, "New action", "Contexts", "project ssh aws bruno plugin"),
    section(Cli, "Shell integration", "install terminal path"),
    section(Cli, "Usage", "editor git commit open"),
    section(Usage, "Overview", "activity stats statistics analytics dashboard profile usage totals sessions tokens commits hours cost spend dollars money"),
    section(Usage, "Calendar", "heatmap contributions streak days year"),
    section(Usage, "By model", "cost price pricing spend opus sonnet gpt models"),
    section(Usage, "By agent", "claude codex share breakdown"),
    section(Usage, "By project", "repositories share breakdown"),
    section(About, "Sikemux", "version build"),
    section(About, "Updates", "version upgrade release"),
    row(About, "Updates", "Channel", "nightly stable prerelease beta"),
    row(About, "Updates", "Last checked", "check for updates now"),
    section(About, "Privacy", "analytics telemetry tracking"),
    row(About, "Privacy", "Share anonymous usage", "analytics telemetry tracking opt out posthog"),
    section(About, "Help", "what's new changelog diagnostics welcome onboarding"),
];

/// Whether `section` of `page` is on this system.
pub fn here(page: Page, section: &str) -> bool {
    page.here() && (cfg!(target_os = "macos") || !MAC_ONLY_SECTIONS.contains(&(page, section)))
}

/// The sections of `page` in the order it shows them, as searches name them.
pub fn sections(page: Page) -> Vec<&'static str> {
    let mut sections: Vec<&'static str> = Vec::new();
    for entry in ENTRIES
        .iter()
        .filter(|entry| entry.page == page && here(page, entry.section))
    {
        if !sections.contains(&entry.section) {
            sections.push(entry.section);
        }
    }
    sections
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
    let mut found: Vec<(usize, usize, Entry)> = ENTRIES
        .iter()
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
