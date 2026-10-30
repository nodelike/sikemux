//! The person's preferences, kept by the core and shared by every client.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_TERMINAL_FONT_SIZE: u32 = 13;
pub const MIN_TERMINAL_FONT_SIZE: u32 = 8;
pub const MAX_TERMINAL_FONT_SIZE: u32 = 32;
/// How much larger than usual a chat's or an editor's text is drawn.
pub const DEFAULT_TEXT_SCALE: f64 = 1.0;
pub const MIN_TEXT_SCALE: f64 = 0.7;
pub const MAX_TEXT_SCALE: f64 = 2.0;
/// The sizes the whole interface's text may be drawn at.
pub const UI_TEXT_SCALES: [f64; 3] = [1.0, 1.1, 1.25];
pub const DEFAULT_THEME_ID: &str = "aura-noir";
/// How many folders below a project root projects are looked for, at most.
pub const MAX_PROJECT_DEPTH: u32 = 8;

/// Where a custom command's output goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CustomCommandPlacement {
    Background,
    Terminal,
    Split,
    Popup,
    Replace,
}

/// A shell command the person added to the command palette.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommand {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
    pub command: String,
    /// The kinds of tab it is offered in; none means every tab.
    #[serde(default)]
    pub contexts: Vec<String>,
    pub placement: CustomCommandPlacement,
}

/// A folder the person keeps projects in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRoot {
    pub path: String,
    /// How many folders below it git repositories count as projects.
    pub depth: u32,
    /// The folder is a project itself, whether or not it is a git repository.
    #[serde(default)]
    pub self_index: bool,
}

/// How the person wants Sikemux to behave, the same in every app. A field
/// left out reads as its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    /// What sending does while an agent's turn runs.
    pub mid_turn_send: MidTurnSend,
    /// One of 1, 1.1 and 1.25.
    pub ui_text_scale: f64,
    pub terminal_font_size: u32,
    pub chat_text_scale: f64,
    pub editor_text_scale: f64,
    /// The theme the person picked, one of the curated, Ghostty or their own.
    pub theme_id: String,
    /// The themes the person made.
    pub custom_themes: Vec<crate::theme::Theme>,
    /// The agent a new chat starts as when none is picked: the last one picked.
    pub last_agent_type: Option<String>,
    /// The projects, by folder, whose new chats start in a worktree of their own.
    pub agent_worktree_defaults: BTreeMap<String, bool>,
    /// Agent tabs open when the app quit open again when it starts.
    pub restore_agent_tabs: bool,
    /// The desktop says when an agent needs the person.
    pub agent_notifications: bool,
    /// The folders the person keeps projects in.
    pub project_roots: Vec<ProjectRoot>,
    /// The repository a project's Git views open on, by the project's folder,
    /// for a project holding more than one.
    pub default_git_repos: BTreeMap<String, String>,
    /// Whether a project's language servers may run, by its folder, once the
    /// person said.
    pub language_server_trust: BTreeMap<String, bool>,
    /// The commands the person added to the command palette.
    pub custom_commands: Vec<CustomCommand>,
    /// Each plugin's own settings, by the plugin's id, as the plugin reads them.
    pub plugin_settings: BTreeMap<String, Value>,
    /// The browser app single sign-on opens in, or none for the default one.
    pub cloud_browser: String,
    /// The shortcut that brings that browser's desktop forward.
    pub cloud_browser_shortcut: String,
    /// Agents may drive the iOS Simulator.
    pub ios_simulator: bool,
    /// How the island over the MacBook notch behaves.
    pub notch: crate::notch::NotchSettings,
    /// Holding right Option dictates; the speech model downloads once it is on.
    pub voice_dictation: bool,
    /// Which releases updates come from.
    pub update_channel: UpdateChannel,
    /// Anonymous usage is reported.
    pub share_usage_data: bool,
    /// The person went through the first-run introduction.
    pub onboarding_complete: bool,
    /// The release whose notes the person last saw.
    pub last_seen_version: String,
    /// The first launch already asked macOS to let Sikemux notify.
    pub notifications_introduced: bool,
    /// The person was told once that terminals keep running after Sikemux quits.
    pub kept_running_notice_shown: bool,
    /// Panes show a dithered texture behind them.
    pub pane_shader: bool,
    /// A picture every pane dithers in place of the texture.
    pub pane_image: Option<String>,
    /// How strongly the texture or picture shows, from 0 to 2.
    pub pane_strength: f64,
    /// The colours the pane picture is dithered in.
    pub pane_colours: PaneColours,
    /// The three inks, dark to light as hex, the picture takes with custom colours.
    pub pane_inks: Vec<String>,
    pub pane_pattern: PanePattern,
    /// How wide each of the picture's dots is, from 1 to 6 points.
    pub pane_dot_size: u32,
    /// From -1 to 1.
    pub pane_contrast: f64,
    /// From -1 to 1.
    pub pane_brightness: f64,
    /// How brightly the picture's light parts glow, from 0 to 1.
    pub pane_glow: f64,
    /// How far the glow spreads, from 0 to 1.
    pub pane_glow_size: f64,
    pub pane_coverage: PaneCoverage,
    /// How opaque the window is, from 0 to 1.
    pub window_opacity: f64,
    /// How much what is behind the window blurs, in points.
    pub window_blur: f64,
    pub rail_density: RailDensity,
    /// The iOS Simulator is drawn inside a device's frame.
    pub simulator_frame: bool,
    /// The keys the person bound to actions in place of their defaults, by
    /// the action's id; none unbinds an action.
    pub keybinding_overrides: BTreeMap<String, Option<String>>,
    /// The side rail is shown.
    pub side_rail_open: bool,
    /// The side rail's width, in points.
    pub side_rail_width: f64,
    /// The tab rail is shown.
    pub tab_rail_open: bool,
    /// The tab rail's width, in points.
    pub tab_rail_width: f64,
    pub tab_rail_page: TabRailPage,
    pub tab_rail_scope: TabRailScope,
    /// The tab rail lists every agent's chats rather than one agent's.
    pub tab_rail_all_agents: bool,
    /// The commands the person ran from the command deck, the last first.
    pub recent_command_keys: Vec<String>,
}

/// The page the tab rail shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabRailPage {
    #[default]
    Agents,
    Terminals,
    Files,
}

/// Whose agents the tab rail lists: the project in front's, or every project's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabRailScope {
    #[default]
    Project,
    All,
}

/// Where the pane picture's inks come from: the theme's ground, accent and ink, the picture's
/// own colours, or the person's three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaneColours {
    #[default]
    Theme,
    Picture,
    Custom,
}

/// The grid that decides which of two inks each dot takes, or random noise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PanePattern {
    Bayer2,
    Bayer4,
    #[default]
    Bayer8,
    Noise,
}

/// How much of a pane the picture covers: a band at its top fading down, or all of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PaneCoverage {
    #[default]
    Band,
    Full,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RailDensity {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Nightly,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            mid_turn_send: MidTurnSend::default(),
            ui_text_scale: UI_TEXT_SCALES[0],
            terminal_font_size: DEFAULT_TERMINAL_FONT_SIZE,
            chat_text_scale: DEFAULT_TEXT_SCALE,
            editor_text_scale: DEFAULT_TEXT_SCALE,
            theme_id: DEFAULT_THEME_ID.to_owned(),
            custom_themes: Vec::new(),
            last_agent_type: None,
            agent_worktree_defaults: BTreeMap::new(),
            restore_agent_tabs: true,
            agent_notifications: true,
            project_roots: Vec::new(),
            default_git_repos: BTreeMap::new(),
            language_server_trust: BTreeMap::new(),
            custom_commands: Vec::new(),
            plugin_settings: BTreeMap::new(),
            cloud_browser: String::new(),
            cloud_browser_shortcut: String::new(),
            ios_simulator: true,
            notch: crate::notch::NotchSettings::default(),
            voice_dictation: false,
            update_channel: UpdateChannel::default(),
            share_usage_data: true,
            onboarding_complete: false,
            last_seen_version: String::new(),
            notifications_introduced: false,
            kept_running_notice_shown: false,
            pane_shader: true,
            pane_image: None,
            pane_strength: 1.0,
            pane_colours: PaneColours::default(),
            pane_inks: vec![
                "#0f1214".to_owned(),
                "#e673af".to_owned(),
                "#fff0fa".to_owned(),
            ],
            pane_pattern: PanePattern::default(),
            pane_dot_size: 2,
            pane_contrast: 0.0,
            pane_brightness: 0.0,
            pane_glow: 0.4,
            pane_glow_size: 0.4,
            pane_coverage: PaneCoverage::default(),
            window_opacity: 1.0,
            window_blur: 0.0,
            rail_density: RailDensity::default(),
            simulator_frame: true,
            keybinding_overrides: BTreeMap::new(),
            side_rail_open: true,
            side_rail_width: 258.0,
            tab_rail_open: true,
            tab_rail_width: 288.0,
            tab_rail_page: TabRailPage::default(),
            tab_rail_scope: TabRailScope::default(),
            tab_rail_all_agents: false,
            recent_command_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MidTurnSend {
    /// The message joins the turn that runs.
    #[default]
    Steer,
    /// The message waits for the turn to end.
    Queue,
}
