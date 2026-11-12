//! Every command the window runs, each declared once: its action and id, what it does, and how
//! it stands with what covers the window. The ids, labels and keys are the shared table's. Which
//! commands the usage report counts is the core's own privacy list, `COMMANDS` in
//! `activity_log/features.rs`, so a new command is counted only once it is added there.

use gpui_kit::{App, Context, Div, InteractiveElement, Stateful, Window};
use sikemux_core::server::call;
use sikemux_view::layout::SplitDir;
use tokio::sync::mpsc;

use super::Workbench;
use crate::core::Ask;
use crate::keymap::{Minimize, RunProjectAction, ToggleFullScreen, Zoom};
use crate::panes::Towards;
use crate::tabs::same_kind;
use sikemux_core::server::calls::NoParams;

/// A command's id, made only by its row below, so naming a command that is not there does not
/// compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandId(&'static str);

impl CommandId {
    pub fn name(self) -> &'static str {
        self.0
    }
}

/// How a command stands with what covers the window.
#[derive(Clone, Copy, PartialEq)]
pub enum Applies {
    /// It still works while a palette, picker or other view with `Modal` in its context has the
    /// keys, as on the Mac: it opens another palette or Settings.
    OverModals,
    /// It leaves Settings open; any other closes it first.
    KeepsSettings,
    /// While Settings covers the window it closes Settings, not a tab or project behind it.
    ClosesSettings,
    /// It acts on a desk's page only while the keys are in it, leaving the same keys to the
    /// editor and everything else.
    OnPage,
}

/// Whether command `id` stands with what covers the window as `applies` says.
pub fn applies(id: &str, applies: Applies) -> bool {
    APPLIES
        .iter()
        .any(|(command, held)| *command == id && held.contains(&applies))
}

/// Runs `run` on the workbench.
fn opening<A: gpui::Action>(
    cx: &Context<Workbench>,
    run: fn(&mut Workbench, &mut Window, &mut Context<Workbench>),
) -> impl Fn(&A, &mut Window, &mut App) + use<A> {
    let this = cx.weak_entity();
    move |_: &A, window: &mut Window, cx: &mut App| {
        this.update(cx, |bench, cx| run(bench, window, cx)).ok();
    }
}

macro_rules! commands {
    ($(
        $(#[$doc:meta])*
        $name:ident = $id:literal $(($($applies:ident),*))? $how:ident $run:expr;
    )*) => {
        $(
            $(#[$doc])*
            #[derive(Clone, Default, PartialEq, gpui::Action)]
            #[action(name = $id)]
            pub struct $name;

            impl $name {
                pub const ID: CommandId = CommandId($id);
            }
        )*

        const APPLIES: &[(&str, &[Applies])] = &[
            $(($id, &[$($(Applies::$applies),*)?]),)*
        ];

        /// Has the window's root run each command.
        pub(super) fn listen(shell: Stateful<Div>, cx: &Context<Workbench>) -> Stateful<Div> {
            shell$(.on_action(commands!(@$how $name, $run, cx)))*
        }
    };
    (@acts $name:ident, $run:expr, $cx:ident) => {
        $cx.listener(|bench, _: &$name, window, cx| {
            let run: fn(&mut Workbench, &mut Window, &mut Context<Workbench>) = $run;
            run(bench, window, cx)
        })
    };
    (@opens $name:ident, $run:expr, $cx:ident) => {
        opening::<$name>($cx, $run)
    };
}

commands! {
    /// Closes the project or host in front, asking first when it would stop agents at
    /// work or lose unsaved files.
    ClosePlace = "session.close" (ClosesSettings) acts |bench, _, cx| bench.close_place(cx);
    /// Goes back to the project or host in front before this one.
    LastPlace = "session.lastUsed" acts |bench, _, cx| bench.go_to_last_place(cx);
    /// Jumps from the projects to the hosts, or back.
    NextGroup = "session.nextGroup" acts |bench, _, cx| bench.next_group(cx);
    /// Lets the agent in front run without asking, or has it ask again.
    ToggleYolo = "agent.permissions" acts |bench, _, _| bench.toggle_yolo_in_front();
    /// The tab rail's terminals.
    FocusTerminals = "session.command" opens |bench, _, _| {
        bench.target().call_typed::<call::ShowTerminals>(NoParams {});
    };
    FocusFiles = "window.files" opens |bench, _, cx| bench.focus_files(cx);
    FocusAgents = "window.agents" opens Workbench::focus_agents;
    FocusHosts = "window.hosts" opens |bench, _, _| bench.open_hosts(None);
    FocusPlaybook = "window.playbook" opens |bench, _, _| bench.open_playbook();
    /// Moves the keys to the next of the window's areas: the rails and the view in front.
    NextArea = "window.nextArea" acts |bench, window, cx| bench.step_area(1, window, cx);
    PreviousArea = "window.previousArea" acts |bench, window, cx| bench.step_area(-1, window, cx);
    NextPlace = "session.next" acts |bench, window, cx| bench.step_place(1, window, cx);
    PreviousPlace = "session.previous" acts |bench, window, cx| bench.step_place(-1, window, cx);
    OpenCommands = "palette.commands" (OverModals) acts |bench, window, cx| {
        bench.toggle_commands(window, cx)
    };
    ChooseAgent = "agent.choose" acts |bench, window, cx| match bench.chosen_host() {
        crate::hosts::HostChoice::Ssh(alias) => bench.open_install_pane(&alias),
        _ => bench.toggle_agent_palette(window, cx),
    };
    OpenProject = "project.open" acts |bench, window, cx| {
        bench.toggle_picker(crate::pickers::picker::Picking::Projects, window, cx)
    };
    OpenHost = "ssh.open" acts |bench, window, cx| {
        bench.toggle_picker(crate::pickers::picker::Picking::Hosts, window, cx)
    };
    OpenPlace = "session.open" acts |bench, window, cx| {
        bench.toggle_picker(crate::pickers::picker::Picking::Both, window, cx)
    };
    NewTab = "palette.newTab" acts |bench, window, cx| bench.toggle_new_tab(window, cx);
    OpenMod = "palette.mods" acts |bench, window, cx| bench.toggle_mods_palette(window, cx);
    SwitchTheme = "theme.switch" acts |bench, window, cx| bench.toggle_theme_palette(window, cx);
    OpenFile = "palette.files" (OverModals) acts |bench, window, cx| {
        bench.toggle_finder(window, cx);
        cx.notify();
    };
    NewTerminal = "terminal.new" opens |bench, _, cx| bench.new_terminal(cx);
    /// Bigger text in the chat, editor or terminal that has the focus.
    TextSizeIncrease = "text.sizeIncrease" (KeepsSettings) acts |bench, _, _| {
        bench.resize_front_text(Some(true))
    };
    TextSizeDecrease = "text.sizeDecrease" (KeepsSettings) acts |bench, _, _| {
        bench.resize_front_text(Some(false))
    };
    TextSizeReset = "text.sizeReset" (KeepsSettings) acts |bench, _, _| {
        bench.resize_front_text(None)
    };
    FocusGit = "window.git" opens |bench, _, cx| bench.open_git(cx);
    /// Searches the project, with the text selected where the keyboard is.
    FocusSearch = "search.global" (OverModals) opens Workbench::open_search;
    FocusSearchWindow = "window.search" opens Workbench::open_search;
    ToggleSettings = "settings.toggle" (OverModals) acts |bench, window, cx| {
        crate::settings::toggle(bench, window, cx)
    };
    /// Starts a chat with the agent the rail is set to.
    NewChat = "agent.new" opens Workbench::new_chat;
    /// Brings the project's terminal forward, opening one when it has none.
    FocusTerminal = "window.terminal" opens |bench, _, cx| bench.focus_terminal(cx);
    ToggleDesk = "desk.toggle" acts |bench, _, cx| bench.toggle_front_desk(cx);
    /// Hides the agent beside its desk, or brings it back.
    DeskAlone = "desk.alone" acts |bench, _, cx| bench.toggle_desk_alone(cx);
    NewPage = "browser.tabNew" opens |bench, _, cx| bench.new_front_page(cx);
    ReopenPage = "browser.tabReopen" opens |bench, _, cx| bench.reopen_front_page(cx);
    FocusAddress = "browser.address" acts |bench, _, cx| bench.focus_front_address(cx);
    /// Each box dragged over the front agent's page goes to the project's front chat.
    Snip = "browser.snip" acts |bench, _, cx| bench.toggle_front_snip(cx);
    ReloadPage = "browser.reload" (OnPage) acts |bench, _, cx| bench.reload_front_page(cx);
    PageBack = "browser.back" (OnPage) acts |bench, _, cx| bench.step_front_page(-1, cx);
    PageForward = "browser.forward" (OnPage) acts |bench, _, cx| bench.step_front_page(1, cx);
    /// The chat that was in front before this one; on a desk's page, ⌘[ goes back in the page.
    ChatBack = "chat.back" acts |bench, _, cx| bench.step_chat(-1, cx);
    ChatForward = "chat.forward" acts |bench, _, cx| bench.step_chat(1, cx);
    NewSimulator = "simulator.new" acts |bench, _, cx| {
        bench.front_simulator(cx);
        bench.parts.redraw_stage(cx);
    };
    PreviousTab = "window.previous" acts |bench, _, _| bench.step_tab(-1, |_, _| true);
    NextTab = "window.next" acts |bench, _, _| bench.step_tab(1, |_, _| true);
    /// The previous tab showing the same kind of pane as the one in front.
    PreviousOfKind = "tab.previous" acts |bench, window, cx| {
        if !bench.step_desk_page(-1, window, cx) {
            bench.step_tab(-1, same_kind);
        }
    };
    NextOfKind = "tab.next" acts |bench, window, cx| {
        if !bench.step_desk_page(1, window, cx) {
            bench.step_tab(1, same_kind);
        }
    };
    GoToTab1 = "tab.goto1" acts |bench, _, _| bench.go_to_tab(Some(1));
    GoToTab2 = "tab.goto2" acts |bench, _, _| bench.go_to_tab(Some(2));
    GoToTab3 = "tab.goto3" acts |bench, _, _| bench.go_to_tab(Some(3));
    GoToTab4 = "tab.goto4" acts |bench, _, _| bench.go_to_tab(Some(4));
    GoToTab5 = "tab.goto5" acts |bench, _, _| bench.go_to_tab(Some(5));
    GoToTab6 = "tab.goto6" acts |bench, _, _| bench.go_to_tab(Some(6));
    GoToTab7 = "tab.goto7" acts |bench, _, _| bench.go_to_tab(Some(7));
    GoToTab8 = "tab.goto8" acts |bench, _, _| bench.go_to_tab(Some(8));
    /// The last tab, wherever it is.
    GoToLastTab = "tab.goto9" acts |bench, _, _| bench.go_to_tab(None);
    CloseTab = "pane.close" (ClosesSettings) acts |bench, window, cx| bench.close_front(window, cx);
    /// Hides both rails.
    FocusMode = "view.focusMode" acts |bench, _, cx| bench.toggle_focus_mode(cx);
    /// A new shell beside the view in focus, to its right or below it, or behind it as a tab.
    SplitRow = "pane.splitRow" acts |bench, _, cx| bench.split_front(SplitDir::Row, cx);
    SplitColumn = "pane.splitColumn" acts |bench, _, cx| bench.split_front(SplitDir::Column, cx);
    SplitStack = "pane.splitStack" acts |bench, _, cx| bench.split_front(SplitDir::Stack, cx);
    FocusLeft = "pane.focusLeft" acts |bench, window, cx| {
        bench.focus_towards(Towards::Left, window, cx)
    };
    FocusRight = "pane.focusRight" acts |bench, window, cx| {
        bench.focus_towards(Towards::Right, window, cx)
    };
    FocusUp = "pane.focusUp" acts |bench, window, cx| {
        bench.focus_towards(Towards::Up, window, cx)
    };
    FocusDown = "pane.focusDown" acts |bench, window, cx| {
        bench.focus_towards(Towards::Down, window, cx)
    };
    ResizeLeft = "pane.resizeLeft" acts |bench, _, _| bench.resize_towards(Towards::Left);
    ResizeRight = "pane.resizeRight" acts |bench, _, _| bench.resize_towards(Towards::Right);
    ResizeUp = "pane.resizeUp" acts |bench, _, _| bench.resize_towards(Towards::Up);
    ResizeDown = "pane.resizeDown" acts |bench, _, _| bench.resize_towards(Towards::Down);
    /// The view in focus fills its tab, or gives the room back.
    ZoomPane = "pane.zoom" acts |bench, _, cx| bench.toggle_zoom(cx);
    /// Stops every agent and terminal the core runs, then quits.
    QuitAndStopEverything = "app.quitAndStopEverything" acts |bench, _, _| {
        let _ = bench.asks.send(Ask::Shutdown);
    };
}

/// What the window runs that takes more than its name, or needs only the window.
pub(super) fn listen_others(shell: Stateful<Div>, cx: &Context<Workbench>) -> Stateful<Div> {
    shell
        .on_action(cx.listener(|bench, action: &RunProjectAction, _, cx| {
            bench.run_command(
                crate::commands::Which::Action(action.id.clone()),
                action.label.clone(),
                cx,
            )
        }))
        .on_action(cx.listener(|bench, open: &crate::editor::OpenAt, _, cx| {
            crate::editor::go(bench, open.clone(), cx)
        }))
        .on_action(|_: &Minimize, window, _| window.minimize_window())
        .on_action(|_: &Zoom, window, _| window.zoom_window())
        .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
}

/// Sets up what each part of the app binds or keeps for the whole app, once.
pub(super) fn install(asks: mpsc::UnboundedSender<Ask>, cx: &mut App) {
    crate::faces::install(cx);
    crate::file_icons::install(cx);
    crate::syntax::install(cx);
    crate::pickers::finder::install(cx);
    crate::palette::install(cx);
    crate::desk::install(cx);
    crate::screens::welcome::install(cx);
    crate::keymap::install(asks, cx);
    crate::chat::install(cx);
    crate::terminal::install(cx);
    crate::files::install(cx);
    crate::ui::keyed::install(cx);
    crate::command_popup::install(cx);
    crate::pairing::install(cx);
    crate::editor::install(cx);
    crate::search::install(cx);
    crate::settings::install(cx);
    crate::pickers::project_switcher::install(cx);
    crate::pickers::host_picker::install(cx);
    crate::chat::model_picker::install(cx);
}
