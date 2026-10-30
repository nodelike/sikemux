use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($icon:ident => $file:literal),* $(,)?) => {
        /// Every icon the app draws, from icons.mjs's SVGs.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($icon),* }

        impl Icon {
            /// The icon drawn from `assets/icons/{file}.svg`, as the core names our tools' icons.
            pub fn named(file: &str) -> Option<Icon> {
                match file { $($file => Some(Icon::$icon),)* _ => None }
            }

            pub fn path(self) -> &'static str {
                match self { $(Icon::$icon => concat!("icons/", $file, ".svg")),* }
            }

            /// Whether it has colours of its own, rather than drawing in the text's.
            pub fn is_picture(self) -> bool {
                match self { $(Icon::$icon => const { !tints(include_bytes!(concat!("../assets/icons/", $file, ".svg"))) }),* }
            }
        }

        const FILES: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/icons/", $file, ".svg")))),*
        ];
    };
}

icons!(
    Actions => "actions",
    Activity => "activity",
    Agent => "agent",
    Agents => "agents",
    ArrowDown => "arrow-down",
    ArrowUp => "arrow-up",
    Aws => "aws",
    BatteryBolt => "battery-bolt",
    BatteryBoltGap => "battery-bolt-gap",
    Board => "board",
    Bitbucket => "bitbucket",
    Bolt => "bolt",
    BoltFill => "bolt-fill",
    Book => "book",
    Briefcase => "briefcase",
    Bruno => "bruno",
    Building => "building",
    Camera => "camera",
    Check => "check",
    Grip => "grip",
    Checkout => "checkout",
    Chevron => "chevron",
    Claude => "claude",
    Click => "click",
    Clock => "clock",
    Close => "close",
    Code => "code",
    Codex => "codex",
    CollapseAll => "collapse-all",
    Command => "command",
    Contrast => "contrast",
    Commit => "commit",
    Copy => "copy",
    Cube => "cube",
    Dashboard => "dashboard",
    Database => "database",
    Discard => "discard",
    Download => "download",
    Editor => "editor",
    ExpandAll => "expand-all",
    Eye => "eye",
    Fetch => "fetch",
    File => "file",
    FilePlus => "file-plus",
    Flask => "flask",
    Focus => "focus",
    Folder => "folder",
    FolderPlus => "folder-plus",
    Git => "git",
    Github => "github",
    Gitlab => "gitlab",
    Globe => "globe",
    Heart => "heart",
    Highlighter => "highlighter",
    Home => "home",
    HomeButton => "home-button",
    Hourglass => "hourglass",
    Gauge => "gauge",
    Image => "image",
    Inbox => "inbox",
    Issue => "issue",
    Issues => "issues",
    Jira => "jira",
    Keyboard => "keyboard",
    Laptop => "laptop",
    Leaf => "leaf",
    Linear => "linear",
    Link => "link",
    Lock => "lock",
    LogIn => "log-in",
    LogLines => "log-lines",
    LogOut => "log-out",
    Logo => "logo",
    MarkCancelled => "mark-cancelled",
    MarkFailure => "mark-failure",
    MarkSkipped => "mark-skipped",
    MarkSuccess => "mark-success",
    Merge => "merge",
    Message => "message",
    Mic => "mic",
    Minus => "minus",
    More => "more",
    Mouse => "mouse",
    Move => "move",
    Music => "music",
    Network => "network",
    Panel => "panel",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    Pencil => "pencil",
    Phone => "phone",
    Pinch => "pinch",
    Plug => "plug",
    Plus => "plus",
    Pin => "pin",
    Pointer => "pointer",
    Pull => "pull",
    Power => "power",
    PullRequest => "pull-request",
    Push => "push",
    Record => "record",
    Refresh => "refresh",
    Releases => "releases",
    Resize => "resize",
    Rocket => "rocket",
    Rotate => "rotate",
    Run => "run",
    Rundeck => "rundeck",
    Search => "search",
    Server => "server",
    Settings => "settings",
    Share => "share",
    Shield => "shield",
    ShieldBolt => "shield-bolt",
    Signoz => "signoz",
    Slack => "slack",
    Sparkle => "sparkle",
    SquarePlus => "square-plus",
    Stack => "stack",
    Star => "star",
    Stash => "stash",
    Status => "status",
    Steps => "steps",
    Stop => "stop",
    Swipe => "swipe",
    Table => "table",
    Tabs => "tabs",
    Tap => "tap",
    Terminal => "terminal",
    Text => "text",
    Timer => "timer",
    TouchPath => "touch-path",
    Trash => "trash",
    Unlink => "unlink",
    Upload => "upload",
    User => "user",
    Vercel => "vercel",
    PhoneQr => "phone-qr",
    Warning => "warning",
    InfoCircle => "info-circle",
    Waterfall => "waterfall",
    Window => "window",
    Workflow => "workflow",
    Wrench => "wrench",
    Grok => "grok",
    Hermes => "hermes",
    Muse => "muse",
    Omp => "omp",
    OpenCode => "opencode",
    Pi => "pi",
    Postgresql => "postgresql",
    Mysql => "mysql",
    Sqlite => "sqlite",
);

/// Whether an SVG draws in `currentColor`.
const fn tints(svg: &[u8]) -> bool {
    let needle = b"currentColor";
    let mut start = 0;
    while start + needle.len() <= svg.len() {
        let mut matched = 0;
        while matched < needle.len() && svg[start + matched] == needle[matched] {
            matched += 1;
        }
        if matched == needle.len() {
            return true;
        }
        start += 1;
    }
    false
}

/// The paths GPUI Kit's widgets draw their icons from, each one of the app's own: a toast's
/// mark for its kind, a submenu's and a select's chevron.
const KIT_ICONS: &[(&str, &[u8])] = &[
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    (
        "icons/circle-check.svg",
        include_bytes!("../assets/icons/circle-check.svg"),
    ),
    (
        "icons/circle-x.svg",
        include_bytes!("../assets/icons/circle-x.svg"),
    ),
    (
        "icons/triangle-alert.svg",
        include_bytes!("../assets/icons/triangle-alert.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../assets/icons/chevron.svg"),
    ),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
];

/// The icons, compiled in.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .chain(KIT_ICONS)
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .chain(KIT_ICONS)
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// An agent's mark.
pub fn agent(provider: &str) -> Icon {
    match provider {
        "codex" => Icon::Codex,
        "claude" => Icon::Claude,
        "grok" => Icon::Grok,
        "hermes" => Icon::Hermes,
        "muse" => Icon::Muse,
        "omp" => Icon::Omp,
        "opencode" => Icon::OpenCode,
        "pi" => Icon::Pi,
        "sikemux" => Icon::Logo,
        _ => Icon::Agent,
    }
}

impl From<Icon> for gpui_kit::component::Icon {
    fn from(icon: Icon) -> Self {
        gpui_kit::component::Icon::default().path(icon.path())
    }
}
