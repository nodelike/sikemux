//! Every page of settings, and every place in them a search can find.

use gpui_kit::App;

use crate::icons::Icon;
use crate::keymap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    General,
    Appearance,
    Keybindings,
    Activity,
    About,
    Agents,
    Actions,
    Cli,
    Devices,
    Cloud,
    Plugins,
}

/// The pages in the list on the left, under their groups.
pub const GROUPS: [(&str, &[Page]); 2] = [
    (
        "App",
        &[
            Page::General,
            Page::Appearance,
            Page::Keybindings,
            Page::Activity,
            Page::About,
        ],
    ),
    (
        "Tools",
        &[
            Page::Agents,
            Page::Actions,
            Page::Cli,
            Page::Devices,
            Page::Cloud,
            Page::Plugins,
        ],
    ),
];

impl Page {
    pub fn name(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Keybindings => "Keybindings",
            Page::Activity => "Activity",
            Page::About => "About",
            Page::Agents => "Agents",
            Page::Actions => "Actions",
            Page::Cli => "Command line",
            Page::Devices => "Devices",
            Page::Cloud => "Cloud",
            Page::Plugins => "Plugins",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Page::General => Icon::Folder,
            Page::Appearance => Icon::Contrast,
            Page::Keybindings => Icon::Command,
            Page::Activity => Icon::Activity,
            Page::About => Icon::InfoCircle,
            Page::Agents => Icon::Agent,
            Page::Actions => Icon::Run,
            Page::Cli => Icon::Editor,
            Page::Devices => Icon::Phone,
            Page::Cloud => Icon::Globe,
            Page::Plugins => Icon::Plug,
        }
    }

    /// The page after or before this one in the list, wrapping around.
    pub fn step(self, forward: bool) -> Page {
        let all: Vec<Page> = GROUPS
            .iter()
            .flat_map(|(_, pages)| pages.iter().copied())
            .collect();
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

use Page::*;

pub const ENTRIES: &[Entry] = &[
    section(Plugins, "Built-in plugins", "aws bitbucket bruno database postgres postgresql mysql sqlite sql github actions gitlab jira rundeck signoz slack enable disable switch off turn on extensions integrations"),
    section(General, "Project folders", "repos repositories directories roots scan depth index picker"),
    section(General, "Session transfer", "send move copy host machine chats tabs another server transfer"),
    section(Appearance, "Theme", "colours colors palette dark light custom fork editor"),
    section(Appearance, "Interface", ""),
    row(Appearance, "Interface", "Text size", "font zoom scale larger smaller accessibility"),
    row(Appearance, "Interface", "Pane texture", "dither dithering shader grain noise background effect"),
    row(Appearance, "Interface", "Pane image", "picture photo wallpaper background dither dithered custom"),
    row(Appearance, "Interface", "Pane strength", "dither grain picture opacity intensity texture image"),
    section(Appearance, "Buddies", "agent icon character avatar mascot face"),
    row(Appearance, "Buddies", "Show buddies", "agent icon character avatar mascot harness logo"),
    row(Appearance, "Buddies", "Pack", "agent icon character avatar mascot style"),
    row(Appearance, "Buddies", "Shape", "agent icon character avatar mixed"),
    row(Appearance, "Buddies", "Animate faces", "motion blink animation"),
    section(Appearance, "Window", ""),
    row(Appearance, "Window", "Opacity", "transparency translucent see-through"),
    row(Appearance, "Window", "Background blur", "vibrancy frosted glass"),
    section(Keybindings, "Shortcuts", "keybindings hotkeys keys keyboard remap"),
    section(Activity, "Overview", "stats statistics analytics dashboard profile usage totals sessions tokens commits hours cost spend dollars money"),
    section(Activity, "Calendar", "heatmap contributions streak days year"),
    section(Activity, "By model", "cost price pricing spend opus sonnet gpt models"),
    section(Activity, "By agent", "claude codex share breakdown"),
    section(Activity, "By project", "repositories share breakdown"),
    section(About, "Updates", "version upgrade release"),
    row(About, "Updates", "Channel", "nightly stable prerelease beta"),
    row(About, "Updates", "Last checked", "check for updates now"),
    section(About, "Privacy", "analytics telemetry tracking"),
    row(About, "Privacy", "Share anonymous usage", "analytics telemetry tracking opt out posthog"),
    section(About, "Help", "what's new changelog diagnostics welcome onboarding"),
    section(Agents, "Agents on this machine", "install reinstall missing claude codex opencode hermes pi omp grok cli"),
    section(Agents, "Launch boundary", "permissions yolo sandbox safety bypass approval"),
    section(Agents, "Claude accounts", "sign in login logout email subscription max pro switch rotate limit add second work personal"),
    section(Agents, "Codex accounts", "sign in login logout email chatgpt plus pro switch rotate limit add second work personal"),
    section(Agents, "Provider profiles", "claude codex accounts"),
    row(Agents, "Provider profiles", "Name", "profile"),
    row(Agents, "Provider profiles", "Provider", "claude codex"),
    row(Agents, "Provider profiles", "Executable path", "binary cli path"),
    row(Agents, "Provider profiles", "Profile directory", "config account home"),
    section(Agents, "Coding plans", "zai glm minimax kimi code opencode go synthetic ollama subscription plan key claude codex"),
    section(Agents, "Model providers", "openrouter baseten together fireworks groq cerebras deepseek moonshot kimi mistral vercel api key hosted open models opencode pi omp claude codex"),
    section(Agents, "Local models", "ollama lm studio lmstudio local offline runner claude codex"),
    section(Agents, "Helpers", "cheap model workers scouts reviewers subagents spawn paid by provider"),
    row(Agents, "Helpers", "Agent", "helper worker scout reviewer subagent cheap model provider"),
    section(Agents, "Skills", "skill SKILL.md library claude codex opencode pi omp hermes grok global own move"),
    row(Agents, "Skills", "Add a skill", "new create install git folder"),
    section(Agents, "Sessions", ""),
    row(Agents, "Sessions", "Restore agent tabs", "resume reopen startup"),
    row(Agents, "Sessions", "Notify when an agent needs you", "notification alert badge dock permission finished"),
    row(Agents, "Sessions", "Test notification", "notification sound permission send try"),
    row(Agents, "Sessions", "Rail density", "compact comfortable sidebar"),
    row(Agents, "Sessions", "Sending while an agent works", "steer queue interrupt enter"),
    row(Agents, "Sessions", "Idle chats", "sleep memory process agents"),
    section(Agents, "Voice", "dictation microphone speech talk transcribe"),
    section(Agents, "Notch", "island dynamic menu bar notch overlay agents running"),
    row(Agents, "Notch", "Show the island", "notch dynamic island enable disable"),
    row(Agents, "Notch", "Show on", "display monitor external screen"),
    row(Agents, "Notch", "Open with", "hover click"),
    row(Agents, "Notch", "In full-screen apps", "fullscreen hide"),
    row(Agents, "Notch", "Peek when", "peek finished needs you"),
    row(Agents, "Notch", "Answer permissions in the notch", "allow deny permission"),
    row(Agents, "Notch", "Sound when an agent needs you", "sound alert"),
    row(Agents, "Notch", "Haptic feedback", "haptics trackpad force touch tick vibration swipe"),
    row(Agents, "Notch", "Step aside for Sikemux Dev", "dev build development"),
    row(Agents, "Voice", "Dictate with right Option", "dictation microphone speech push to talk hold"),
    row(Agents, "Voice", "Speech model", "parakeet download neural engine"),
    section(Agents, "iOS Simulator", "iphone ipad xcode devicehub simulator phone mobile bezel frame buttons"),
    row(Agents, "iOS Simulator", "Let agents drive the iOS Simulator", "iphone tap swipe app test mobile"),
    row(Agents, "iOS Simulator", "Xcode", "xcode-select developer tools"),
    row(Agents, "iOS Simulator", "iOS runtimes", "ios version runtime"),
    row(Agents, "iOS Simulator", "Simulator helper", "download idb"),
    section(Actions, "Your actions", "custom commands command deck scripts"),
    section(Actions, "New action", "custom command add create"),
    row(Actions, "New action", "Name", "title"),
    row(Actions, "New action", "Description", "detail"),
    row(Actions, "New action", "Command", "shell script"),
    row(Actions, "New action", "Where output lands", "placement terminal split popup background"),
    row(Actions, "New action", "Contexts", "project ssh aws bruno plugin"),
    section(Cli, "Shell integration", "install terminal path"),
    section(Cli, "Usage", "editor git commit open"),
    section(Devices, "Your account", "account sign in sign out login google github email delete"),
    section(Devices, "Remote access", "phone mobile iphone android remote away connect host enable turn on off"),
    section(Devices, "Phones", "phone mobile iphone android qr code scan revoke remove forget access watch full control"),
    section(Devices, "Hosts", "linked hosts machines servers rename remove account link"),
    section(Cloud, "Single sign-on", "sso aws gcp login"),
    row(Cloud, "Single sign-on", "Browser app", "chrome safari firefox arc"),
    row(Cloud, "Single sign-on", "Workspace shortcut", "desktop space mission control"),
];

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
            page: Keybindings,
            section: "Shortcuts",
            label: shortcut.label,
            keywords: shortcut.detail,
            shortcut: Some(shortcut.id),
        };
        (entry, format!("shortcut {keys}"))
    });
    let mut found: Vec<(usize, usize, Entry)> = ENTRIES
        .iter()
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
