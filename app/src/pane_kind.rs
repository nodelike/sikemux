//! What the app does with each kind of view, in one place: what it is called and drawn with,
//! where the rail opens it, its page on the tab rail, whether its menu moves it out of a split
//! into a tab of its own, how it is made and drawn, and how it takes the keys.

use crate::workbench::actions;
use crate::workbench::actions::CommandId;
use gpui_kit::{
    div, AnyElement, App, Context, Focusable, Hsla, IntoElement, ParentElement, Styled, Window,
};
use sikemux_view::layout::PaneKind;
use sikemux_view::prefs::TabRailPage;
use sikemux_wire::protocol::SessionId;

use crate::icons::Icon;
use crate::theme::{Tokens, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

pub struct Kind {
    pub icon: Icon,
    /// What a tab of it is called when nothing in it names it.
    pub label: &'static str,
    pub separable: Separable,
    /// Whether a swipe over its tab pages between tabs; one that scrolls sideways itself keeps
    /// the gesture.
    pub swipe_pages: bool,
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
    let kind_of = |icon, label, separable, swipe_pages| Kind {
        icon,
        label,
        separable,
        swipe_pages,
    };
    use Separable::{Always, Never, WhenSaved};
    match kind {
        PaneKind::Agent => kind_of(Icon::Agent, "Agent", Never, true),
        PaneKind::Terminal => kind_of(Icon::Terminal, "Terminal", Always, true),
        PaneKind::Files => kind_of(Icon::Editor, "Files", WhenSaved, false),
        PaneKind::Git => kind_of(Icon::Git, "Git", Always, true),
        PaneKind::Search => kind_of(Icon::Search, "Search", Always, true),
        PaneKind::Board => kind_of(Icon::Board, "Agent board", Never, true),
        PaneKind::Desk => kind_of(Icon::PanelRight, "Desk", Never, false),
        PaneKind::ProjectSettings => kind_of(Icon::Settings, "Project settings", Never, true),
        PaneKind::InstallSikemux => kind_of(Icon::Download, "Install Sikemux", Never, true),
        PaneKind::Stash => kind_of(Icon::Stack, "Stash", Never, true),
        PaneKind::Hosts => kind_of(Icon::Gauge, "Hosts", Never, true),
        PaneKind::Mod => kind_of(Icon::Dashboard, "Mod", Always, false),
        PaneKind::Unknown => kind_of(Icon::Window, "Needs a newer Sikemux", Never, true),
    }
}

/// A surface of each project on the side rail, in the order it lists them.
pub struct Surface {
    pub kind: PaneKind,
    pub label: &'static str,
    pub command: CommandId,
}

pub const SURFACES: [Surface; 6] = [
    Surface {
        kind: PaneKind::Files,
        label: "Files",
        command: actions::FocusFiles::ID,
    },
    Surface {
        kind: PaneKind::Terminal,
        label: "Terminal",
        command: actions::FocusTerminal::ID,
    },
    Surface {
        kind: PaneKind::Agent,
        label: "Agents",
        command: actions::FocusAgents::ID,
    },
    Surface {
        kind: PaneKind::Git,
        label: "Git",
        command: actions::FocusGit::ID,
    },
    Surface {
        kind: PaneKind::Search,
        label: "Search",
        command: actions::FocusSearch::ID,
    },
    Surface {
        kind: PaneKind::Board,
        label: "Agent board",
        command: actions::FocusBoard::ID,
    },
];

/// The tab rail's pages in the order they sit, left to right.
pub const PAGES: [TabRailPage; 3] = [
    TabRailPage::Agents,
    TabRailPage::Terminals,
    TabRailPage::Files,
];

/// The kind whose views a tab rail page lists.
pub fn listed_on(page: TabRailPage) -> PaneKind {
    match page {
        TabRailPage::Agents => PaneKind::Agent,
        TabRailPage::Terminals => PaneKind::Terminal,
        TabRailPage::Files => PaneKind::Files,
    }
}

/// The tab rail's page that lists views of `kind`.
pub fn page_of(kind: PaneKind) -> Option<TabRailPage> {
    PAGES.into_iter().find(|page| listed_on(*page) == kind)
}

/// The colour of its icon on a tab: a terminal's is lit, as it runs something.
pub fn tint(kind: PaneKind, t: &Tokens) -> Hsla {
    match kind {
        PaneKind::Terminal => t.live,
        _ => t.text_tertiary,
    }
}

/// Makes what a view of `kind` needs, as its tab comes to the front. A mod's and a desk's panes
/// are made for the whole strip by [`Workbench::settle_front`], which lets go of those it no
/// longer shows; the rest make themselves as they draw or hold nothing to make.
pub fn make(
    kind: PaneKind,
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    match kind {
        PaneKind::Git => bench.git_pane(window, cx),
        PaneKind::Search => bench.search_pane(cx),
        PaneKind::Board => bench.board_pane(cx),
        PaneKind::Agent
        | PaneKind::Terminal
        | PaneKind::Files
        | PaneKind::Desk
        | PaneKind::ProjectSettings
        | PaneKind::InstallSikemux
        | PaneKind::Stash
        | PaneKind::Hosts
        | PaneKind::Mod
        | PaneKind::Unknown => {}
    }
}

/// Draws the view `id` of `tab`, a `kind` showing `target`.
pub fn draw(
    kind: PaneKind,
    bench: &Workbench,
    tab: &str,
    id: &str,
    target: &str,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    match kind {
        PaneKind::Terminal => ui::pane(cx)
            .child(crate::terminal::pane(
                bench,
                &cx.entity(),
                &bench.place(),
                target,
                cx,
            ))
            .into_any_element(),
        PaneKind::Files => ui::pane(cx)
            .child(crate::editor::pane(bench, &cx.entity(), target, window, cx))
            .into_any_element(),
        PaneKind::Git => crate::git::pane(bench, window, cx),
        PaneKind::Search => match &bench.search {
            Some(pane) => ui::pane(cx).child(pane.clone()).into_any_element(),
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Board => crate::board::pane(bench, window, cx),
        PaneKind::Mod => match bench.mod_panes.get(id) {
            Some(pane) => ui::pane(cx).child(pane.clone()).into_any_element(),
            None => ui::pane(cx).into_any_element(),
        },
        PaneKind::Agent => crate::home::agent_view(bench, tab, id, target, cx),
        PaneKind::Desk => crate::desk::pane(bench, target, window, cx),
        PaneKind::ProjectSettings => crate::project_settings::pane(bench, target, window, cx),
        PaneKind::InstallSikemux => crate::hosts::install_pane::pane(bench, target, cx),
        PaneKind::Stash => crate::stash::pane(bench, cx),
        PaneKind::Hosts => crate::hosts::page::pane(bench, target, window, cx),
        PaneKind::Unknown => ui::pane(cx)
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(Tokens::get(cx).text_tertiary)
                    .child("This tab needs a newer Sikemux."),
            )
            .into_any_element(),
    }
}

/// Puts the keys in a view of `kind` showing `target`, or else in the window.
pub fn focus(kind: PaneKind, bench: &Workbench, target: &str, window: &mut Window, cx: &mut App) {
    match kind {
        PaneKind::Terminal => {
            let focus = target.parse::<SessionId>().ok().and_then(|session| {
                let key = bench.terminals.key(&bench.place(), session);
                bench.terminals.focus(key)
            });
            if let Some(focus) = focus {
                window.focus(focus, cx);
            }
        }
        PaneKind::Agent => {
            if let Some(composing) =
                (bench.chats.open.get(target)).and_then(|chat| chat.composer.as_ref())
            {
                composing.draft.read(cx).focus_handle(cx).focus(window, cx);
            }
        }
        PaneKind::Files
        | PaneKind::Git
        | PaneKind::Search
        | PaneKind::Board
        | PaneKind::Desk
        | PaneKind::ProjectSettings
        | PaneKind::InstallSikemux
        | PaneKind::Stash
        | PaneKind::Hosts
        | PaneKind::Mod
        | PaneKind::Unknown => bench.keys.focus.focus(window, cx),
    }
}
