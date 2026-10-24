//! Every agent Sikemux can set up here, as the core lists them, and installs and
//! sign-ins run with the agent's own command in a sheet where the person can watch
//! it, from Settings or the welcome screen.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, point, px, AnchoredPositionMode, AnyElement, Context, Entity, Hsla,
    InteractiveElement, IntoElement, ParentElement, Styled, Window,
};
use serde_json::{json, Value};
use sikemux_core::agent_accounts::status::AgentStatus;
use sikemux_wire::protocol::{SessionId, TerminalSpawn};
use tokio::sync::oneshot;

use crate::core::{self, Ask, TerminalAsk};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

/// One agent as the core found it.
pub struct Found {
    pub kind: String,
    pub label: String,
    pub about: String,
    install: Option<String>,
    login: Option<Vec<String>>,
    /// Where its program is, once found.
    program: Option<String>,
    pub status: AgentStatus,
}

/// How far the sheet's command got.
#[derive(Clone)]
enum Phase {
    Running,
    Checking,
    Done(Option<AgentStatus>),
    Failed(Option<u32>),
    /// The terminal would not start.
    Refused(String),
}

/// An install or sign-in running where the person can watch it.
#[derive(Clone)]
pub struct Sheet {
    install: bool,
    kind: String,
    label: String,
    shown: String,
    /// What the shell runs; it exits with the command's code.
    command: String,
    session: Option<SessionId>,
    phase: Phase,
}

/// The agents found, once the core has said, and the sheet while one shows.
#[derive(Default)]
pub struct Setup {
    pub found: Option<Vec<Found>>,
    sheet: Option<Sheet>,
}

/// What draws the agents and the sheet: Settings on the stage, or the welcome over it.
fn redraw(bench: &Workbench, cx: &mut Context<Workbench>) {
    bench.parts.redraw(&bench.parts.stage, cx);
}

/// What the agent `kind` finds itself to be, on the account it runs on now.
async fn availability(
    asks: &tokio::sync::mpsc::UnboundedSender<Ask>,
    kind: &str,
    config: Option<String>,
) -> (AgentStatus, Option<String>) {
    let asked = json!({ "provider": kind, "configPath": config });
    let answer = core::request(asks, "agentAvailability", asked)
        .await
        .unwrap_or_default();
    let status = serde_json::from_value(answer["status"].clone()).unwrap_or(AgentStatus::Missing);
    (status, answer["program"].as_str().map(str::to_owned))
}

/// Asks the core which agents are here, with those it can install that are not.
pub fn read(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let listed = core::request(&asks, "agentSetup", json!({}))
            .await
            .unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            let catalog = bench.read("catalog").and_then(Value::as_array).cloned();
            let found = listed
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|agent| {
                    let kind = agent["type"].as_str().unwrap_or_default().to_owned();
                    let entry = catalog
                        .iter()
                        .flatten()
                        .find(|entry| entry["type"] == kind.as_str());
                    let field = |name: &str| entry.and_then(|entry| entry[name].as_str());
                    Found {
                        label: agent["label"].as_str().unwrap_or_default().to_owned(),
                        about: field("about").unwrap_or_default().to_owned(),
                        install: field("install").map(str::to_owned),
                        login: entry
                            .and_then(|entry| entry["login"].as_array())
                            .map(|args| {
                                args.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            }),
                        program: (agent["available"] == true)
                            .then(|| agent["command"].as_str().map(str::to_owned))
                            .flatten(),
                        status: serde_json::from_value(agent["status"].clone())
                            .unwrap_or(AgentStatus::Missing),
                        kind,
                    }
                })
                .collect();
            bench.agent_setup.found = Some(found);
            redraw(bench, cx);
        });
    })
    .detach();
}

pub fn status_label(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Missing => "Not installed",
        AgentStatus::Broken { .. } => "Not working",
        AgentStatus::SignedOut => "Signed out",
        AgentStatus::Ready { account } => match account.as_deref() {
            Some("apiKey") => "Ready · API key",
            Some("subscription") => "Ready · subscription",
            _ => "Ready",
        },
        AgentStatus::Unknown => "Found",
    }
}

/// The folder an account keeps its sign-in in: the path itself, or the folder of the
/// config file it names.
fn account_folder(path: &str) -> String {
    let home = sikemux_process::paths::home()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = match path.trim().strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None => path.trim().to_owned(),
    };
    let config = [
        "config.toml",
        "config.yml",
        "settings.json",
        "settings.local.json",
    ];
    let named = std::path::Path::new(&path);
    match named.file_name().and_then(|name| name.to_str()) {
        Some(name) if config.contains(&name) => named
            .parent()
            .map(|folder| folder.to_string_lossy().into_owned())
            .unwrap_or(path.clone()),
        _ => path.clone(),
    }
}

/// `word` as the shell reads it back whole.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The installer as the shell runs it.
pub fn installing(agent: &Found) -> Option<Sheet> {
    Some(installer(&agent.kind, &agent.label, agent.install.clone()?))
}

/// The installer `command` of the agent `kind`, as the shell runs it.
pub fn installer(kind: &str, label: &str, command: String) -> Sheet {
    Sheet {
        install: true,
        kind: kind.to_owned(),
        label: label.to_owned(),
        shown: command.clone(),
        command: format!("{command}\nexit $?"),
        session: None,
        phase: Phase::Running,
    }
}

/// The agent's own sign-in, on the account folder it runs on now.
pub fn signing_in(agent: &Found, folder: Option<&str>) -> Option<Sheet> {
    let args = agent.login.clone()?;
    let program = agent.program.clone().unwrap_or_else(|| agent.kind.clone());
    let shown = std::iter::once(agent.kind.clone())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let variable = match agent.kind.as_str() {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        _ => None,
    };
    let words: Vec<String> = std::iter::once(program)
        .chain(args)
        .map(|word| quoted(&word))
        .collect();
    let account = match (variable, folder) {
        (Some(variable), Some(folder)) => {
            format!("{variable}={} ", quoted(&account_folder(folder)))
        }
        _ => String::new(),
    };
    let command = format!("{account}{}\nexit $?", words.join(" "));
    Some(Sheet {
        install: false,
        kind: agent.kind.clone(),
        label: agent.label.clone(),
        shown,
        command,
        session: None,
        phase: Phase::Running,
    })
}

/// Starts the sheet's command in a terminal of its own.
pub fn run(bench: &mut Workbench, sheet: Sheet, cx: &mut Context<Workbench>) {
    let spawn = TerminalSpawn {
        cols: 100,
        rows: 26,
        cwd: sikemux_process::paths::home().map(|home| home.to_string_lossy().into_owned()),
        startup: Some(sheet.command.clone()),
        ..TerminalSpawn::default()
    };
    bench.agent_setup.sheet = Some(sheet);
    let (started, session) = oneshot::channel();
    let _ = bench.asks.send(Ask::Spawn {
        spawn: Box::new(spawn),
        started,
    });
    cx.spawn(async move |bench, cx| {
        let session = session.await;
        let _ = bench.update(cx, |bench, cx| {
            if let Some(sheet) = bench.agent_setup.sheet.as_mut() {
                match session {
                    Ok(Ok(session)) => sheet.session = Some(session),
                    Ok(Err(error)) => sheet.phase = Phase::Refused(error),
                    Err(_) => sheet.phase = Phase::Refused("the core is not connected".into()),
                }
            }
            redraw(bench, cx);
        });
    })
    .detach();
    redraw(bench, cx);
}

/// Moves the sheet on once its command ended: looks for the agent again after it worked.
fn ended(bench: &mut Workbench, code: Option<u32>, cx: &mut Context<Workbench>) {
    let Some(sheet) = bench.agent_setup.sheet.as_mut() else {
        return;
    };
    if !matches!(sheet.phase, Phase::Running) {
        return;
    }
    if code != Some(0) {
        sheet.phase = Phase::Failed(code);
        return redraw(bench, cx);
    }
    sheet.phase = Phase::Checking;
    let kind = sheet.kind.clone();
    let config = bench
        .accounts
        .current(&kind)
        .and_then(|profile| profile.config_path.clone());
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let _ = core::request(&asks, "forgetAgentStatus", json!({})).await;
        let (status, _) = availability(&asks, &kind, config).await;
        let _ = bench.update(cx, |bench, cx| {
            if let Some(sheet) = bench.agent_setup.sheet.as_mut() {
                let found = !matches!(status, AgentStatus::Missing);
                sheet.phase = Phase::Done(found.then_some(status));
            }
            read(bench, cx);
        });
    })
    .detach();
    redraw(bench, cx);
}

/// Closes the sheet, ending its command when it still runs.
fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(sheet) = bench.agent_setup.sheet.take() else {
        return;
    };
    if let (Some(session), Phase::Running) = (sheet.session, &sheet.phase) {
        let _ = bench.asks.send(Ask::Terminal(session, TerminalAsk::Kill));
    }
    redraw(bench, cx);
}

/// Runs the sheet's command again after it failed.
fn again(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(mut sheet) = bench.agent_setup.sheet.take() else {
        return;
    };
    sheet.session = None;
    sheet.phase = Phase::Running;
    run(bench, sheet, cx);
}

fn state(sheet: &Sheet) -> String {
    let label = &sheet.label;
    match (&sheet.phase, sheet.install) {
        (Phase::Running, true) => format!("Running {label}'s own installer…"),
        (Phase::Running, false) => format!("Running {label}'s own sign-in. Finish it in the browser if one opened."),
        (Phase::Checking, true) => format!("Installed. Looking for {label}…"),
        (Phase::Checking, false) => format!("Checking {label}'s sign-in…"),
        (Phase::Refused(error), _) => format!("Sikemux could not start it: {error}"),
        (Phase::Failed(code), install) => format!(
            "The {} stopped{}. Its output above says why.",
            if install { "installer" } else { "sign-in" },
            code.map(|code| format!(" with exit code {code}")).unwrap_or_default()
        ),
        (Phase::Done(None), true) => format!(
            "The installer finished, but Sikemux can't find {label} yet. Open a new terminal or restart Sikemux."
        ),
        (Phase::Done(Some(status)), true) => {
            format!("{label} is installed: {}.", status_label(status).to_lowercase())
        }
        (Phase::Done(Some(status @ AgentStatus::Ready { .. })), false) => {
            format!("{label} is signed in: {}.", status_label(status).to_lowercase())
        }
        (Phase::Done(Some(AgentStatus::SignedOut)), false) => {
            format!("{label} still reads as signed out. Try again, or check the output above.")
        }
        (Phase::Done(_), false) => format!("The sign-in finished. Start a {label} chat to use it."),
    }
}

/// The sheet over the window while a command runs in it.
pub fn sheet(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let sheet = bench.agent_setup.sheet.as_ref()?;
    let t = Tokens::get(cx).clone();
    if let (Phase::Running, Some(session)) = (&sheet.phase, sheet.session) {
        let exit = bench
            .view
            .as_ref()
            .and_then(|view| view.sessions.iter().find(|each| each.id == session))
            .and_then(|each| each.exit.clone());
        if let Some(exit) = exit {
            cx.defer_in(window, move |bench, _, cx| ended(bench, exit.code, cx));
        }
    }
    let title = match sheet.install {
        true => format!("Install {}", sheet.label),
        false => format!("Sign in to {}", sheet.label),
    };
    let terminal: AnyElement = match sheet.session {
        Some(session) => crate::terminal::pane(
            bench,
            this,
            &crate::hosts::Place::This,
            &session.to_string(),
            cx,
        ),
        None => div().into_any_element(),
    };
    let button = |id: &'static str,
                  label: &'static str,
                  run: fn(&mut Workbench, &mut Context<Workbench>)| {
        let this = this.clone();
        ui::Button::new(id, label, ui::Variant::Outline)
            .on_click(move |_, _, cx| this.update(cx, run))
    };
    let failed = matches!(sheet.phase, Phase::Failed(_) | Phase::Refused(_));
    let running = matches!(sheet.phase, Phase::Running);
    let card = div()
        .id("agent-sheet")
        .w(px(760.))
        .h(px(520.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_4)
        .shadow(t.shadow_float())
        .occlude()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(16.))
                .py(px(10.))
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(t.text_tertiary)
                        .child(sheet.shown.clone()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .bg(t.surface_sunken)
                .child(terminal),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(10.))
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(crate::theme::ui_text(12.))
                        .text_color(t.text_secondary)
                        .child(state(sheet)),
                )
                .when(failed, |foot| {
                    foot.child(button("agent-sheet-again", "Try again", again))
                })
                .child(button(
                    "agent-sheet-close",
                    if running { "Cancel" } else { "Close" },
                    close,
                )),
        );
    let size = window.viewport_size();
    Some(
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .w(size.width)
                        .h(size.height)
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Hsla {
                            h: 0.,
                            s: 0.,
                            l: 0.,
                            a: 0.55,
                        })
                        .child(card),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}
