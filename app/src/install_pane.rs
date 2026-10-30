//! The Install Sikemux pane of an SSH host (ADR 0019): what setting Sikemux up there adds,
//! and the one action that does it, as the core says the host needs.

use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled,
};

use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_2H, TEXT_MD, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// What setting an SSH host up takes, as the core's host row names it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetUp {
    Install,
    SignIn,
    Start,
    Update,
}

impl SetUp {
    pub fn from_work(work: &str) -> Option<Self> {
        match work {
            "install" => Some(Self::Install),
            "signIn" => Some(Self::SignIn),
            "start" => Some(Self::Start),
            "update" => Some(Self::Update),
            _ => None,
        }
    }

    pub fn work(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::SignIn => "signIn",
            Self::Start => "start",
            Self::Update => "update",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::SignIn => Icon::LogIn,
            Self::Start => Icon::Power,
            Self::Install | Self::Update => Icon::Download,
        }
    }

    /// What the button that does it says.
    pub fn offer(self, alias: &str) -> String {
        match self {
            Self::Install => format!("Install Sikemux on {alias}"),
            Self::SignIn => format!("Sign in {alias}"),
            Self::Start => format!("Start Sikemux on {alias}"),
            Self::Update => format!("Update Sikemux on {alias}"),
        }
    }

    /// What a host's menu calls it.
    pub fn menu_label(self) -> &'static str {
        match self {
            Self::Install => "Install Sikemux",
            Self::SignIn => "Sign in",
            Self::Start => "Start Sikemux",
            Self::Update => "Update Sikemux",
        }
    }

    /// What it says while it runs.
    pub fn running(self, alias: &str) -> String {
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
                "Sikemux is on {alias} but not signed in. Signing in runs sikemux login there, in a new tab, and adds it to your account."
            ),
            Self::Start => {
                format!("Sikemux is on {alias} but its core is stopped. Starting runs sikemux start there.")
            }
            Self::Update => format!(
                "{alias} runs a release of Sikemux too old for this app. Updating runs the installer over your SSH in a new tab, then restarts its core."
            ),
        }
    }
}

impl Workbench {
    /// The SSH host `alias`'s row, as the core lists it.
    fn ssh_row(&self, alias: &str) -> Option<&serde_json::Value> {
        self.reads
            .hosts
            .as_ref()?
            .as_array()?
            .iter()
            .find(|host| host["ssh"] == alias)
    }

    /// What setting `alias` up takes, if anything does.
    pub fn set_up_of(&self, alias: &str) -> Option<SetUp> {
        SetUp::from_work(self.ssh_row(alias)?["setUp"].as_str()?)
    }

    /// The set-up running on `alias` now, if one is.
    pub fn setting_up(&self, alias: &str) -> Option<SetUp> {
        let setup = &self.ssh_row(alias)?["setup"];
        setup["problem"]
            .is_null()
            .then(|| SetUp::from_work(setup["work"].as_str()?))
            .flatten()
    }

    /// Opens the Install Sikemux pane in the SSH host's strip, or brings it forward.
    pub fn open_install_pane(&self, alias: &str) {
        self.call(
            "openTab",
            crate::workbench::Strip::Host(alias.to_owned()).with(serde_json::json!({
                "kind": "installSikemux",
                "target": alias,
            })),
        );
    }
}

/// The pane for the SSH host `alias`.
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
    let Some(set_up) = bench.set_up_of(alias) else {
        return ui::pane(cx)
            .child(note(
                Icon::Download,
                None,
                format!("{alias} runs Sikemux, on your account or another one."),
            ))
            .into_any_element();
    };
    let shown = note(
        set_up.icon(),
        Some(set_up.offer(alias)),
        match bench.setting_up(alias) {
            Some(running) => format!(
                "{} A password, if {alias} asks for one, goes in the tab beside this one.",
                running.running(alias)
            ),
            None => set_up.says(alias),
        },
    );
    let shown = match bench.setting_up(alias) {
        Some(_) => shown,
        None => {
            let alias = alias.to_owned();
            shown.child(
                ui::Button::new(
                    SharedString::from(format!("set-up-{alias}")),
                    set_up.offer(&alias),
                    ui::Variant::Primary,
                )
                .on_click(
                    cx.listener(move |bench, _, _, cx| bench.set_up_host(&alias, set_up, cx)),
                ),
            )
        }
    };
    ui::pane(cx).child(shown).into_any_element()
}
