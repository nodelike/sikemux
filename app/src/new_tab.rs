//! New…: every kind of tab the window can open, each by the command that opens
//! it, so a kind joins the list once the window can do its command.

use gpui_kit::{Context, Window};

use sikemux_view::layout::PaneKind;

use crate::hosts::HostChoice;
use crate::icons::Icon;
use crate::keymap;
use crate::palette::{Mark, Row, Spec};
use crate::workbench::{Strip, Workbench};

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "palette.newTab")]
pub struct NewTab;

struct Choice {
    command: &'static str,
    label: &'static str,
    detail: &'static str,
    icon: Icon,
    /// Opens only in a project, not on an SSH host.
    in_project: bool,
}

impl Choice {
    /// A tab of `kind`, called and marked as such tabs are.
    fn of(kind: PaneKind, command: &'static str, detail: &'static str, in_project: bool) -> Self {
        let kind = crate::pane_kind::of(kind);
        Self {
            command,
            label: kind.label,
            detail,
            icon: kind.icon,
            in_project,
        }
    }
}

fn choices() -> [Choice; 9] {
    [
        Choice::of(PaneKind::Terminal, "terminal.new", "A new shell", false),
        Choice::of(
            PaneKind::Agent,
            "agent.choose",
            "Start or resume an agent",
            true,
        ),
        Choice {
            command: "browser.tabNew",
            label: "Browser",
            detail: "A page on the agent's desk",
            icon: Icon::Globe,
            in_project: true,
        },
        Choice {
            command: "simulator.new",
            label: "iOS Simulator",
            detail: "Drive an iPhone on the agent's desk",
            icon: Icon::Phone,
            in_project: true,
        },
        Choice::of(
            PaneKind::Files,
            "palette.files",
            "Open a project file",
            true,
        ),
        Choice::of(
            PaneKind::Git,
            "window.git",
            "Changes, branches, remotes and stashes",
            true,
        ),
        Choice::of(
            PaneKind::Search,
            "search.global",
            "Search across the project",
            true,
        ),
        Choice::of(
            PaneKind::Board,
            "window.board",
            "What the project's agents are doing",
            true,
        ),
        Choice::of(
            PaneKind::Mod,
            "palette.mods",
            "A panel you or an agent made",
            false,
        ),
    ]
}

/// The row that sets Sikemux up on the SSH host in front.
const INSTALL: &str = "host.install";

impl Workbench {
    pub fn toggle_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An SSH host without Sikemux is a shell: a login there, or installing Sikemux (ADR 0019).
        if let HostChoice::Ssh(alias) = self.chosen_host() {
            return self.toggle_ssh_new_tab(alias, window, cx);
        }
        let in_project = matches!(self.strip(), Some(Strip::Project(_)));
        let rows = choices()
            .iter()
            .filter(|choice| cx.build_action(choice.command, None).is_ok())
            .filter(|choice| cfg!(target_os = "macos") || choice.command != "simulator.new")
            .map(|choice| {
                let shut = choice.in_project && !in_project;
                Row::new(choice.command, choice.label)
                    .meta(if shut {
                        "open a project first"
                    } else {
                        choice.detail
                    })
                    .keys(keymap::hint(choice.command, cx))
                    .mark(Mark::Icon(choice.icon))
                    .disabled(shut)
            })
            .collect();
        let spec = Spec::new(
            "palette.newTab",
            "open a new tab…",
            rows,
            |row, _, window, cx| {
                keymap::dispatch(&row.key, window, cx);
            },
        )
        .numbered()
        .width(440.);
        self.toggle_palette(spec, window, cx);
    }

    fn toggle_ssh_new_tab(&mut self, alias: String, window: &mut Window, cx: &mut Context<Self>) {
        let rows = vec![
            Row::new("terminal.new", "SSH terminal")
                .meta(format!("A new login on {alias}"))
                .keys(keymap::hint("terminal.new", cx))
                .mark(Mark::Icon(Icon::Terminal)),
            Row::new(INSTALL, "Install Sikemux")
                .meta(format!("What Sikemux adds on {alias}, and the installer"))
                .keys(keymap::hint("agent.new", cx))
                .mark(Mark::Icon(Icon::Download)),
        ];
        let spec = Spec::new(
            "palette.newTab",
            "open a new tab…",
            rows,
            move |row, bench, window, cx| match row.key.as_ref() {
                INSTALL => bench.open_install_pane(&alias),
                command => keymap::dispatch(command, window, cx),
            },
        )
        .width(440.);
        self.toggle_palette(spec, window, cx);
    }
}
