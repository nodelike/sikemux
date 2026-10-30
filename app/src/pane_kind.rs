//! What the app does with each kind of view, in one place: what it is called and drawn with,
//! where the rail opens it, its page on the tab rail, and whether its menu moves it out of a
//! split into a tab of its own.

use sikemux_wire::protocol::PaneKind;

use crate::icons::Icon;
use crate::workbench::RailPage;

pub struct Kind {
    pub icon: Icon,
    /// What a tab of it is called when nothing in it names it.
    pub label: &'static str,
    /// A surface of each project on the side rail: what the rail calls it and the command it runs.
    pub surface: Option<(&'static str, &'static str)>,
    /// The tab rail's page that lists views of it.
    pub page: Option<RailPage>,
    pub separable: Separable,
}

/// Whether a split's view of the kind can move out into a tab of its own.
#[derive(Clone, Copy, PartialEq)]
pub enum Separable {
    Always,
    /// Once what it shows is saved, so nothing unsaved is torn away.
    WhenSaved,
    Never,
}

pub fn of(kind: PaneKind) -> Kind {
    let kind_of = |icon, label, surface, page, separable| Kind {
        icon,
        label,
        surface,
        page,
        separable,
    };
    use Separable::{Always, Never, WhenSaved};
    match kind {
        PaneKind::Agent => kind_of(
            Icon::Agent,
            "Agent",
            Some(("Agents", "window.agents")),
            Some(RailPage::Agents),
            Never,
        ),
        PaneKind::Terminal => kind_of(
            Icon::Terminal,
            "Terminal",
            Some(("Terminal", "window.terminal")),
            Some(RailPage::Terminals),
            Always,
        ),
        PaneKind::Files => kind_of(
            Icon::Editor,
            "Files",
            Some(("Files", "window.files")),
            Some(RailPage::Files),
            WhenSaved,
        ),
        PaneKind::Git => kind_of(Icon::Git, "Git", Some(("Git", "window.git")), None, Always),
        PaneKind::Search => kind_of(
            Icon::Search,
            "Search",
            Some(("Search", "search.global")),
            None,
            Always,
        ),
        PaneKind::Board => kind_of(
            Icon::Board,
            "Agent board",
            Some(("Agent board", "window.board")),
            None,
            Never,
        ),
        PaneKind::Browser => kind_of(Icon::Globe, "Browser", None, None, Never),
        PaneKind::Plugin => kind_of(Icon::Window, "Plugin", None, None, Never),
        PaneKind::Desk => kind_of(Icon::PanelRight, "Desk", None, None, Never),
        PaneKind::ProjectSettings => kind_of(Icon::Settings, "Project settings", None, None, Never),
        PaneKind::InstallSikemux => kind_of(Icon::Download, "Install Sikemux", None, None, Never),
        PaneKind::Stash => kind_of(Icon::Stack, "Stash", None, None, Never),
        PaneKind::Unknown => kind_of(Icon::Window, "Needs a newer Sikemux", None, None, Never),
    }
}
