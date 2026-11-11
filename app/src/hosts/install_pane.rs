//! The Install Sikemux pane of an SSH host (ADR 0019): what setting Sikemux up there adds,
//! and the one action that does it, as the core says the host needs.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::server::hosts::{HostRow, HostUnanswered as Unanswered};
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_2H, TEXT_MD, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

pub use sikemux_core::server::hosts::SetUpWork as SetUp;

/// What the window says of setting an SSH host up.
pub trait Words {
    fn icon(self) -> Icon;
    fn offer(self, alias: &str) -> String;
    fn menu_label(self) -> &'static str;
    fn running(self, alias: &str) -> String;
    fn says(self, alias: &str) -> String;
}

impl Words for SetUp {
    fn icon(self) -> Icon {
        match self {
            Self::SignIn => Icon::LogIn,
            Self::Start => Icon::Power,
            Self::Install | Self::Update => Icon::Download,
        }
    }

    /// What the button that does it says.
    fn offer(self, alias: &str) -> String {
        match self {
            Self::Install => format!("Install Sikemux on {alias}"),
            Self::SignIn => format!("Sign in {alias}"),
            Self::Start => format!("Start Sikemux on {alias}"),
            Self::Update => format!("Update Sikemux on {alias}"),
        }
    }

    /// What a host's menu calls it.
    fn menu_label(self) -> &'static str {
        match self {
            Self::Install => "Install Sikemux",
            Self::SignIn => "Sign in",
            Self::Start => "Start Sikemux",
            Self::Update => "Update Sikemux",
        }
    }

    /// What it says while it runs.
    fn running(self, alias: &str) -> String {
        match self {
            Self::Install => format!("Installing {alias}…"),
            Self::SignIn => format!("Signing in {alias}…"),
            Self::Start => format!("Starting {alias}…"),
            Self::Update => format!("Updating {alias}…"),
        }
    }

    fn says(self, alias: &str) -> String {
        match self {
            Self::Install => format!(
                "With Sikemux, {alias} runs agents that keep working while you are away, your phone reaches it, and its projects, chats and files open here without SSH. The installer runs over your SSH in a new tab, where you type sudo's password if it asks."
            ),
            Self::SignIn => format!(
                "Sikemux is on {alias} but not signed in. Signing in runs sikemux login there over your SSH and adds it to your account."
            ),
            Self::Start => {
                format!("Sikemux is on {alias} but its core is stopped. Starting runs sikemux start there.")
            }
            Self::Update => format!(
                "{alias} runs a release of Sikemux too old for this app. Updating runs its own update over your SSH, on the channel it follows, and restarts its core."
            ),
        }
    }
}

/// What the window says of an SSH host the core has no answer from yet.
pub fn unanswered_says(unanswered: &Unanswered, alias: &str) -> String {
    match unanswered {
        Unanswered::Checking => format!("Checking what runs on {alias}…"),
        Unanswered::Unreachable { detail } => {
            format!("{alias} can't be reached over SSH: {detail}")
        }
        Unanswered::SshSignIn { detail } => format!(
            "SSH needs a password or key to reach {alias}, so Sikemux can't check it: {detail}"
        ),
        Unanswered::Failed { detail } => {
            format!("Sikemux couldn't tell what runs on {alias}: {detail}")
        }
    }
}

/// The short words a rail row says of it: of Sikemux's check, not of the terminal, which may
/// well be connected.
pub fn unanswered_detail(unanswered: &Unanswered) -> &'static str {
    match unanswered {
        Unanswered::Checking => "checking",
        Unanswered::Unreachable { .. } | Unanswered::Failed { .. } => "can't check",
        Unanswered::SshSignIn { .. } => "needs ssh key",
    }
}

impl Workbench {
    /// The SSH host `alias`'s row, as the core lists it.
    pub(crate) fn ssh_row(&self, alias: &str) -> Option<&HostRow> {
        self.reads
            .hosts
            .iter()
            .find(|host| host.ssh.as_deref() == Some(alias))
    }

    /// What setting `alias` up takes, if anything does.
    pub fn set_up_of(&self, alias: &str) -> Option<SetUp> {
        self.ssh_row(alias)?.set_up
    }

    /// The set-up running on `alias` now, if one is.
    pub fn setting_up(&self, alias: &str) -> Option<SetUp> {
        let setup = self.ssh_row(alias)?.setup.as_ref()?;
        setup.problem.is_none().then_some(setup.work)
    }

    /// Why the last set-up of `alias` stopped short, and what it was, until it is dismissed.
    pub fn set_up_problem(&self, alias: &str) -> Option<(SetUp, String)> {
        let setup = self.ssh_row(alias)?.setup.as_ref()?;
        Some((setup.work, setup.problem.clone()?))
    }

    /// The set-up `alias` is offered: the one its newest answer calls for, or a retry of the one
    /// that stopped short while there is no answer yet to say otherwise.
    pub fn set_up_offered(&self, alias: &str) -> Option<SetUp> {
        let row = self.ssh_row(alias)?;
        let unknown =
            row.found.is_none() && matches!(row.unanswered, None | Some(Unanswered::Checking));
        row.set_up.or_else(|| {
            let setup = row.setup.as_ref()?;
            (setup.problem.is_some() && unknown).then_some(setup.work)
        })
    }

    /// What the tab rail and the new-tab list call the pane, as `alias` needs.
    pub fn set_up_label(&self, alias: &str) -> (Icon, &'static str) {
        match self.setting_up(alias).or_else(|| self.set_up_of(alias)) {
            Some(set_up) => (set_up.icon(), set_up.menu_label()),
            None => (Icon::Server, "Sikemux here"),
        }
    }

    /// Opens the Install Sikemux pane in the SSH host's strip, or brings it forward.
    pub fn open_install_pane(&self, alias: &str) {
        self.open_here(OpenTab::new(
            At::host(alias),
            PaneKind::InstallSikemux,
            alias,
        ));
    }
}

pub fn pane(bench: &Workbench, alias: &str, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let note = |icon: Icon, title: Option<String>, message: String| {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACE_2H)
            .px(px(40.))
            .child(ui::icon(icon, 22.).text_color(t.text_tertiary))
            .children(title.map(|title| {
                ui::text(title, TEXT_MD, FontWeight::SEMIBOLD).text_color(t.text_primary)
            }))
            .child(
                div()
                    .max_w(px(460.))
                    .text_center()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_secondary)
                    .child(message),
            )
    };
    let row = bench.ssh_row(alias);
    let problem = bench.set_up_problem(alias);
    let Some(set_up) = (bench.setting_up(alias)).or(bench.set_up_offered(alias)) else {
        let says = match row.and_then(|row| row.unanswered.as_ref()) {
            Some(unanswered) => unanswered_says(unanswered, alias),
            None => format!("{alias} runs Sikemux, on your account or another one."),
        };
        let says = match &problem {
            Some((work, problem)) => {
                format!(
                    "{says} The last {} stopped short: {problem}",
                    work.menu_label()
                )
            }
            None => says,
        };
        return ui::pane(cx)
            .child(note(Icon::Server, None, says))
            .into_any_element();
    };
    let shown = note(
        set_up.icon(),
        Some(set_up.offer(alias)),
        match (bench.setting_up(alias), &problem) {
            (Some(SetUp::Install), _) => format!(
                "{} A password, if {alias} asks for one, goes in the tab beside this one.",
                SetUp::Install.running(alias)
            ),
            (Some(running), _) => running.running(alias),
            (None, Some((work, problem))) => {
                format!("{} stopped short: {problem}", work.menu_label())
            }
            (None, None) => set_up.says(alias),
        },
    );
    let shown = match bench.setting_up(alias) {
        Some(_) => shown,
        None => {
            let (retry, dismiss) = (alias.to_owned(), alias.to_owned());
            shown.child(
                div()
                    .flex()
                    .gap(SPACE_2H)
                    .child(
                        ui::Button::new(
                            SharedString::from(format!("set-up-{alias}")),
                            set_up.offer(alias),
                            ui::Variant::Primary,
                        )
                        .on_click(
                            cx.listener(move |bench, _, _, cx| {
                                bench.set_up_host(&retry, set_up, cx)
                            }),
                        ),
                    )
                    .when(problem.is_some(), |buttons| {
                        buttons.child(
                            ui::Button::new(
                                SharedString::from(format!("dismiss-set-up-{alias}")),
                                "Dismiss",
                                ui::Variant::Ghost,
                            )
                            .on_click(
                                cx.listener(move |bench, _, _, _| bench.dismiss_set_up(&dismiss)),
                            ),
                        )
                    }),
            )
        }
    };
    ui::pane(cx).child(shown).into_any_element()
}
