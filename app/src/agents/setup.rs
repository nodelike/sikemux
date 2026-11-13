//! Every agent Sikemux can set up here, as the core lists them, and installs and
//! sign-ins run with the agent's own command where the person can watch it: in a
//! sheet over Settings, or under the list of agents in the first-run setup.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    anchored, deferred, div, point, px, AnchoredPositionMode, AnyElement, Context, Div, Entity,
    FocusHandle, Hsla, InteractiveElement, IntoElement, ParentElement, Stateful, Styled, Window,
};
use sikemux_core::agent_accounts::status::{AgentStatus, FREE_MODELS};
use sikemux_core::server::agent_accounts::{Account, Forget, Installed};
use sikemux_core::server::agent_updates::{UpdateAgents, Updating};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_view::prefs::PreferenceChange;
use sikemux_wire::protocol::{SessionId, TerminalSpawn};
use tokio::sync::oneshot;

use crate::core::{Ask, Target, TerminalAsk};
use crate::hosts::Place;
use crate::theme::{
    self, Tokens, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

pub struct Found {
    pub kind: String,
    pub label: String,
    pub about: String,
    /// It runs free models with no account or key.
    pub free_models: bool,
    install: Option<String>,
    login: Option<Vec<String>>,
    /// Where its program is, once found.
    program: Option<String>,
    pub status: AgentStatus,
    /// The release installed, the releases its chats run, and the newest out.
    pub version: Option<String>,
    pub running: Vec<String>,
    pub latest: Option<String>,
    pub updating: Option<Updating>,
    /// The models updating lets it run.
    pub unlocks: Vec<String>,
}

impl Found {
    /// The newer release it can update to, while it is not updating already.
    pub fn update_to(&self) -> Option<&str> {
        let (version, latest) = (self.version.as_deref()?, self.latest.as_deref()?);
        let ready = !matches!(
            self.status,
            AgentStatus::Missing | AgentStatus::Broken { .. }
        );
        (ready && sikemux_core::server::agent_updates::is_newer(latest, version)).then_some(latest)
    }

    /// Whether an update of it waits or runs.
    pub fn updating_now(&self) -> bool {
        matches!(
            self.updating,
            Some(Updating::Queued | Updating::Running { .. })
        )
    }
}

/// The agents on an account host, as its core lists them.
pub struct Elsewhere {
    pub host: String,
    pub found: Vec<Found>,
}

/// How many exits are kept in case a spawn's reply is still on its way.
const MAX_EXITS_KEPT: usize = 16;

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
    /// The core has listed its session, so its leaving the list means it ended.
    seen: bool,
    phase: Phase,
}

/// Which of its two jobs a sheet is at.
#[derive(Clone, Copy, PartialEq)]
pub enum Doing {
    Installing,
    SigningIn,
}

impl Sheet {
    /// What it runs, as the person would read it.
    pub fn shown_command(&self) -> &str {
        &self.shown
    }

    fn doing(&self) -> Doing {
        match self.install {
            true => Doing::Installing,
            false => Doing::SigningIn,
        }
    }
}

/// The agents found, once the core has said, the sheet while one shows, and the installs
/// started from a chat's model menu or the add-provider dialog, which run with no sheet.
#[derive(Default)]
pub struct Setup {
    pub found: Option<Vec<Found>>,
    /// The agents on each account host reached, for updating them from here.
    pub elsewhere: Vec<Elsewhere>,
    sheet: Option<Sheet>,
    behind: Vec<Sheet>,
    /// An install or sign-in ended, however it did, since this was last cleared.
    pub ended: bool,
    /// Status is being read again at the person's asking.
    pub checking: bool,
    /// A read is due shortly, to follow an account host's update.
    following: bool,
    /// The last exits heard, for a command whose spawn reply came after its exit.
    exits: Vec<(SessionId, Option<u32>)>,
}

/// How an install running with no sheet is going.
#[derive(Clone, Copy, PartialEq)]
pub enum Installing {
    Running,
    Failed,
}

impl Setup {
    /// The agents found on the host at `place`, once its core has said.
    pub fn on(&self, place: &Place) -> Option<&[Found]> {
        match place {
            Place::This => self.found.as_deref(),
            Place::Account(host) => (self.elsewhere.iter())
                .find(|each| &each.host == host)
                .map(|each| each.found.as_slice()),
        }
    }

    /// Whether the host at `place` has listed its agents.
    pub fn knows(&self, place: &Place) -> bool {
        self.on(place).is_some()
    }

    /// The sheet running `session`, shown or not.
    fn running(&mut self, session: SessionId) -> Option<&mut Sheet> {
        self.sheet
            .iter_mut()
            .chain(self.behind.iter_mut())
            .find(|sheet| sheet.session == Some(session))
    }

    /// What the sheet is doing to `kind`, while it still is.
    pub fn underway(&self, kind: &str) -> Option<Doing> {
        let sheet = self.sheet.as_ref().filter(|sheet| sheet.kind == kind)?;
        matches!(sheet.phase, Phase::Running | Phase::Checking).then(|| sheet.doing())
    }

    /// What the sheet tried to do to `kind`, when it failed and is still up.
    pub fn failure(&self, kind: &str) -> Option<Doing> {
        let sheet = self.sheet.as_ref().filter(|sheet| sheet.kind == kind)?;
        matches!(sheet.phase, Phase::Failed(_) | Phase::Refused(_)).then(|| sheet.doing())
    }

    /// Whether the sheet shows `session`, whose last output stays readable after it ends.
    pub fn shows(&self, session: SessionId) -> bool {
        self.sheet
            .as_ref()
            .is_some_and(|sheet| sheet.session == Some(session))
    }

    /// How the sheet's command stands, in a sentence.
    pub fn sheet_state(&self) -> Option<String> {
        self.sheet.as_ref().map(state)
    }

    /// Whether a sheet is up, running or not.
    pub fn shown(&self) -> bool {
        self.sheet.is_some()
    }

    /// Whether the sheet's command stopped short.
    pub fn sheet_failed(&self) -> bool {
        self.sheet
            .as_ref()
            .is_some_and(|sheet| matches!(sheet.phase, Phase::Failed(_) | Phase::Refused(_)))
    }

    /// Whether the sheet's command is still going, which another must wait for.
    pub fn busy(&self) -> bool {
        self.sheet
            .as_ref()
            .is_some_and(|sheet| matches!(sheet.phase, Phase::Running | Phase::Checking))
    }

    /// Forgets the sheet once its command is over, leaving one still running to Settings.
    pub fn drop_finished(&mut self) {
        if !self.busy() {
            self.sheet = None;
        }
    }

    /// How the install of `kind` started with no sheet is going.
    pub fn installing(&self, kind: &str) -> Option<Installing> {
        let sheet = self.behind.iter().find(|sheet| sheet.kind == kind)?;
        Some(match sheet.phase {
            Phase::Running | Phase::Checking => Installing::Running,
            _ => Installing::Failed,
        })
    }
}

/// Draws the agents and the sheet again, on Settings or the welcome over the stage, and lets
/// the messages that waited for an install go once it ended.
fn redraw(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.parts.redraw_stage(cx);
    crate::chat::installs_moved(bench, cx);
}

/// What the agent `kind` finds itself to be, on the account it runs on now.
async fn availability(
    asks: &tokio::sync::mpsc::UnboundedSender<Ask>,
    kind: &str,
    config: Option<String>,
) -> (AgentStatus, Option<String>) {
    let asked = Target::here(asks).request_typed::<call::AgentAvailability>(Account {
        provider: kind.to_owned(),
        config_path: config,
    });
    match asked.await {
        Ok(found) => (
            found.status,
            found
                .program
                .map(|program| program.to_string_lossy().into_owned()),
        ),
        Err(_) => (AgentStatus::Missing, None),
    }
}

/// What the core's listing says of each agent, with what the catalog adds.
fn found_of(bench: &Workbench, listed: Vec<Installed>) -> Vec<Found> {
    listed
        .into_iter()
        .map(|agent| {
            let entry = bench.reads.catalog.kind(&agent.kind);
            Found {
                about: entry.map(|entry| entry.about.clone()).unwrap_or_default(),
                free_models: entry.is_some_and(|entry| entry.free_models),
                install: entry.and_then(|entry| entry.install.clone()),
                login: entry.and_then(|entry| entry.login.clone()),
                program: agent.available.then_some(agent.command),
                status: agent.status,
                label: agent.label,
                kind: agent.kind,
                version: agent.version,
                running: agent.running,
                latest: agent.latest,
                updating: agent.updating,
                unlocks: agent.unlocks,
            }
        })
        .collect()
}

/// Asks the core which agents are here, with those it can install that are not, and each
/// account host reached which agents it has.
pub fn read(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    let hosts: Vec<String> = (bench.feeds.hosts())
        .filter(|(_, feed)| feed.reached)
        .map(|(host, _)| host.clone())
        .collect();
    cx.spawn(async move |bench, cx| {
        let here = Target::here(&asks).request_typed::<call::AgentSetup>(NoParams {});
        let asked: Vec<_> = (hosts.into_iter())
            .map(|host| {
                let asked = Target::new(&asks, Place::Account(host.clone()))
                    .request_typed::<call::AgentSetup>(NoParams {});
                (host, asked)
            })
            .collect();
        let listed = here.await.unwrap_or_default();
        let mut there = Vec::new();
        for (host, asked) in asked {
            there.push((host, asked.await));
        }
        let _ = bench.update(cx, |bench, cx| {
            let found = found_of(bench, listed);
            let elsewhere = there
                .into_iter()
                .filter_map(|(host, listed)| {
                    let mut found = found_of(bench, listed.ok()?);
                    found.retain(|agent| agent.status != AgentStatus::Missing);
                    Some(Elsewhere { host, found })
                })
                .collect();
            let was_updating = updating_kinds(&bench.agent_setup);
            bench.agent_setup.checking = false;
            bench.agent_setup.found = Some(found);
            bench.agent_setup.elsewhere = elsewhere;
            tell_updated(bench, was_updating, cx);
            follow_updates(bench, cx);
            redraw(bench, cx);
        });
    })
    .detach();
}

/// Each agent whose update waits or runs, by the host it is on, none for this one.
fn updating_kinds(setup: &Setup) -> Vec<(Option<String>, String)> {
    let here = (setup.found.iter().flatten()).map(|agent| (None, agent));
    let there = (setup.elsewhere.iter()).flat_map(|host| {
        host.found
            .iter()
            .map(|agent| (Some(host.host.clone()), agent))
    });
    here.chain(there)
        .filter(|(_, agent)| agent.updating_now())
        .map(|(host, agent)| (host, agent.kind.clone()))
        .collect()
}

/// Says which updates finished since the last read, and how.
fn tell_updated(
    bench: &mut Workbench,
    was: Vec<(Option<String>, String)>,
    cx: &mut Context<Workbench>,
) {
    for (host, kind) in was {
        let found = match &host {
            None => bench.agent_setup.found.as_deref(),
            Some(host) => (bench.agent_setup.elsewhere.iter())
                .find(|each| &each.host == host)
                .map(|each| each.found.as_slice()),
        };
        let Some(agent) = found.and_then(|found| found.iter().find(|agent| agent.kind == kind))
        else {
            continue;
        };
        if agent.updating_now() {
            continue;
        }
        let on = host.map(|host| bench.host_name(&host));
        let agent_name = &agent.label;
        let note = match (&agent.updating, &agent.version, on) {
            (Some(Updating::Failed { .. }), _, None) => {
                crate::toast::error(t!("{agent} did not update", agent = agent_name))
            }
            (Some(Updating::Failed { .. }), _, Some(host)) => crate::toast::error(t!(
                "{agent} did not update on {host}",
                agent = agent_name,
                host = host
            )),
            (_, Some(version), None) => crate::toast::success(t!(
                "{agent} updated to {version}",
                agent = agent_name,
                version = version
            )),
            (_, Some(version), Some(host)) => crate::toast::success(t!(
                "{agent} updated to {version} on {host}",
                agent = agent_name,
                version = version,
                host = host
            )),
            (_, None, None) => crate::toast::success(t!("{agent} updated", agent = agent_name)),
            (_, None, Some(host)) => crate::toast::success(t!(
                "{agent} updated on {host}",
                agent = agent_name,
                host = host
            )),
        };
        bench.notify(note, cx);
    }
}

/// Reads again shortly while an account host updates an agent, as only this host's core
/// tells the window when its agents change.
fn follow_updates(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let elsewhere =
        (bench.agent_setup.elsewhere.iter()).any(|host| host.found.iter().any(Found::updating_now));
    if !elsewhere || bench.agent_setup.following {
        return;
    }
    bench.agent_setup.following = true;
    cx.spawn(async move |bench, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(2))
            .await;
        let _ = bench.update(cx, |bench, cx| {
            bench.agent_setup.following = false;
            read(bench, cx);
        });
    })
    .detach();
}

/// Updates the agents `kinds` on the host at `place`, or every one with a newer release out.
pub fn update(
    bench: &mut Workbench,
    place: Place,
    kinds: Vec<String>,
    cx: &mut Context<Workbench>,
) {
    let asked =
        Target::new(&bench.asks, place).request_typed::<call::UpdateAgents>(UpdateAgents { kinds });
    cx.spawn(async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            if let Err(error) = answer {
                bench.notify(
                    crate::toast::error(t!("Could not update: {error}", error = error)),
                    cx,
                );
            }
            read(bench, cx);
        });
    })
    .detach();
}

/// Updates every agent with a newer release out, on this host and every account host reached.
pub fn update_everywhere(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let hosts: Vec<String> = (bench.agent_setup.elsewhere.iter())
        .filter(|host| host.found.iter().any(|agent| agent.update_to().is_some()))
        .map(|host| host.host.clone())
        .collect();
    for place in std::iter::once(Place::This).chain(hosts.into_iter().map(Place::Account)) {
        update(bench, place, Vec::new(), cx);
    }
}

/// Whether the agent can be started, as far as its own check can tell.
pub fn usable(status: &AgentStatus) -> bool {
    matches!(status, AgentStatus::Ready { .. } | AgentStatus::Unknown)
}

/// What the agent needs next, as the button that does it says, and the sheet that does it.
pub fn next_step(bench: &Workbench, agent: &Found) -> Option<(&'static str, Sheet)> {
    match agent.status {
        AgentStatus::Missing => installing(agent).map(|sheet| (t!("Install"), sheet)),
        AgentStatus::Broken { .. } => installing(agent).map(|sheet| (t!("Reinstall"), sheet)),
        AgentStatus::SignedOut => {
            let folder = bench
                .accounts
                .current(&agent.kind)
                .and_then(|profile| profile.config_path.clone());
            signing_in(agent, folder.as_deref()).map(|sheet| (t!("Sign in"), sheet))
        }
        _ => None,
    }
}

pub fn status_label(status: &AgentStatus) -> &'static str {
    match status {
        AgentStatus::Missing => t!("Not installed"),
        AgentStatus::Broken { .. } => t!("Not working"),
        AgentStatus::SignedOut => t!("Signed out"),
        AgentStatus::Ready { account } => match account.as_deref() {
            Some("apiKey") => t!("Ready · API key"),
            Some("subscription") => t!("Ready · subscription"),
            Some(FREE_MODELS) => t!("Ready · free models"),
            _ => t!("Ready"),
        },
        AgentStatus::Unknown => t!("Found"),
    }
}

/// `word` as the shell reads it back whole.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

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
        seen: false,
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
    let variable = sikemux_core::launch::account_variable(&agent.kind);
    let words: Vec<String> = std::iter::once(program)
        .chain(args)
        .map(|word| quoted(&word))
        .collect();
    let account = match (variable, folder) {
        (Some(variable), Some(folder)) => {
            format!(
                "{variable}={} ",
                quoted(&sikemux_process::paths::account_dir(folder).to_string_lossy())
            )
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
        seen: false,
        phase: Phase::Running,
    })
}

/// Starts the sheet's command in a terminal of its own, in the sheet over the window.
pub fn run(bench: &mut Workbench, sheet: Sheet, cx: &mut Context<Workbench>) {
    bench.agent_setup.sheet = Some(sheet.clone());
    start(bench, sheet, cx);
}

/// Installs the agent `kind` with its own installer, with no sheet over the window, unless an
/// install of it already runs.
pub fn install_behind(bench: &mut Workbench, kind: &str, cx: &mut Context<Workbench>) {
    if bench.agent_setup.installing(kind) == Some(Installing::Running) {
        return;
    }
    let Some(sheet) = catalog_installer(bench, kind) else {
        return;
    };
    bench.agent_setup.behind.retain(|each| each.kind != kind);
    bench.agent_setup.behind.push(sheet.clone());
    start(bench, sheet, cx);
}

/// Shows the install of `kind` that ran with no sheet in Settings › Agents, where its output
/// says why it stopped.
pub fn show_install(
    bench: &mut Workbench,
    kind: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let Some(at) = bench
        .agent_setup
        .behind
        .iter()
        .position(|each| each.kind == kind)
    else {
        return;
    };
    let sheet = bench.agent_setup.behind.remove(at);
    bench.agent_setup.sheet = Some(sheet);
    bench.open_settings(crate::settings::Page::Installed, window, cx);
    redraw(bench, cx);
}

fn catalog_installer(bench: &Workbench, kind: &str) -> Option<Sheet> {
    let entry = bench.reads.catalog.kind(kind)?;
    Some(installer(kind, &entry.label, entry.install.clone()?))
}

fn start(bench: &mut Workbench, sheet: Sheet, cx: &mut Context<Workbench>) {
    let spawn = TerminalSpawn {
        cols: 100,
        rows: 26,
        cwd: sikemux_process::paths::home().map(|home| home.to_string_lossy().into_owned()),
        startup: Some(sheet.command.clone()),
        ..TerminalSpawn::default()
    };
    let (started, session) = oneshot::channel();
    let _ = bench.asks.send(Ask::Spawn {
        spawn: Box::new(spawn),
        started,
    });
    let kind = sheet.kind;
    cx.spawn(async move |bench, cx| {
        let session = session.await;
        let _ = bench.update(cx, |bench, cx| {
            let setup = &mut bench.agent_setup;
            let waiting = setup
                .sheet
                .iter_mut()
                .chain(setup.behind.iter_mut())
                .find(|sheet| sheet.kind == kind && sheet.session.is_none());
            let refused = match (waiting, session) {
                (Some(sheet), Ok(Ok(session))) => {
                    sheet.session = Some(session);
                    false
                }
                (Some(sheet), Ok(Err(error))) => {
                    sheet.phase = Phase::Refused(error);
                    true
                }
                (Some(sheet), Err(_)) => {
                    sheet.phase = Phase::Refused(t!("the core is not connected").to_owned());
                    true
                }
                (None, Ok(Ok(session))) => {
                    let _ = bench.asks.send(Ask::Terminal(session, TerminalAsk::Kill));
                    false
                }
                (None, _) => false,
            };
            if refused {
                note_tried(bench);
            }
            settle(bench, cx);
            redraw(bench, cx);
        });
    })
    .detach();
    redraw(bench, cx);
}

/// Remembers that a step has ended, across a relaunch, so the setup can offer a way past it.
fn note_tried(bench: &mut Workbench) {
    bench.agent_setup.ended = true;
    let preferences = bench.preferences();
    if !preferences.onboarding_complete && !preferences.onboarding_tried {
        crate::settings::set(bench, PreferenceChange::OnboardingTried(true));
    }
}

/// Reads again where every agent stands, past what the core last heard.
pub fn recheck(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.agent_setup.checking = true;
    redraw(bench, cx);
    let forgot =
        Target::here(&bench.asks).request_typed::<call::ForgetAgentStatus>(Forget::default());
    cx.spawn(async move |bench, cx| {
        let _ = forgot.await;
        let _ = bench.update(cx, |bench, cx| read(bench, cx));
    })
    .detach();
}

/// Hears that a terminal's process ended, which comes before the core drops it from the list.
pub fn exited(
    bench: &mut Workbench,
    session: SessionId,
    code: Option<u32>,
    cx: &mut Context<Workbench>,
) {
    let setup = &mut bench.agent_setup;
    setup.exits.push((session, code));
    let over = setup.exits.len().saturating_sub(MAX_EXITS_KEPT);
    setup.exits.drain(..over);
    settle(bench, cx);
}

/// Moves on every sheet whose command ended, as the core's view says. A session the core has
/// dropped without saying how it ended counts as stopped, so the sheet never waits forever.
pub fn settle(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let listed: Vec<(SessionId, Option<Option<u32>>)> = bench
        .this_view()
        .map_or(&[][..], |view| view.sessions.as_slice())
        .iter()
        .map(|session| (session.id, session.exit.as_ref().map(|exit| exit.code)))
        .collect();
    let has_view = bench.this_view().is_some();
    let setup = &mut bench.agent_setup;
    let exits = setup.exits.clone();
    let mut over = Vec::new();
    for sheet in setup.sheet.iter_mut().chain(setup.behind.iter_mut()) {
        let (Some(session), Phase::Running) = (sheet.session, &sheet.phase) else {
            continue;
        };
        let heard = exits.iter().find(|(id, _)| *id == session);
        match (heard, listed.iter().find(|(id, _)| *id == session)) {
            (Some((_, code)), _) => over.push((session, *code)),
            (None, Some((_, Some(code)))) => over.push((session, *code)),
            (None, Some(_)) => sheet.seen = true,
            (None, None) if sheet.seen && has_view => over.push((session, None)),
            (None, None) => {}
        }
    }
    for (session, code) in over {
        self::ended(bench, session, code, cx);
    }
}

/// Moves the sheet on once its command ended: looks for the agent again after it worked.
fn ended(
    bench: &mut Workbench,
    session: SessionId,
    code: Option<u32>,
    cx: &mut Context<Workbench>,
) {
    let Some(sheet) = bench.agent_setup.running(session) else {
        return;
    };
    if code != Some(0) {
        sheet.phase = Phase::Failed(code);
        note_tried(bench);
        read(bench, cx);
        return redraw(bench, cx);
    }
    sheet.phase = Phase::Checking;
    let kind = sheet.kind.clone();
    note_tried(bench);
    let config = bench
        .accounts
        .current(&kind)
        .and_then(|profile| profile.config_path.clone());
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let forgot =
            Target::here(&asks).request_typed::<call::ForgetAgentStatus>(Forget::default());
        let _ = forgot.await;
        let (status, _) = availability(&asks, &kind, config).await;
        let _ = bench.update(cx, |bench, cx| {
            let found = !matches!(status, AgentStatus::Missing);
            let setup = &mut bench.agent_setup;
            if found {
                setup.behind.retain(|sheet| sheet.session != Some(session));
            }
            if let Some(sheet) = setup.running(session) {
                sheet.phase = Phase::Done(found.then_some(status));
            }
            read(bench, cx);
        });
    })
    .detach();
    redraw(bench, cx);
}

/// Closes the sheet, ending its command when it still runs.
pub fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(sheet) = bench.agent_setup.sheet.take() else {
        return;
    };
    if matches!(sheet.phase, Phase::Running | Phase::Checking) {
        note_tried(bench);
    }
    if let (Some(session), Phase::Running) = (sheet.session, &sheet.phase) {
        let _ = bench.asks.send(Ask::Terminal(session, TerminalAsk::Kill));
    }
    redraw(bench, cx);
}

fn again(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(mut sheet) = bench.agent_setup.sheet.take() else {
        return;
    };
    sheet.session = None;
    sheet.seen = false;
    sheet.phase = Phase::Running;
    run(bench, sheet, cx);
}

fn state(sheet: &Sheet) -> String {
    let label = &sheet.label;
    match (&sheet.phase, sheet.install) {
        (Phase::Running, true) => t!("Running {label}'s own installer…", label = label),
        (Phase::Running, false) => t!(
            "Running {label}'s own sign-in. Finish it in the browser if one opened.",
            label = label
        ),
        (Phase::Checking, true) => t!("Installed. Looking for {label}…", label = label),
        (Phase::Checking, false) => t!("Checking {label}'s sign-in…", label = label),
        (Phase::Refused(error), _) => t!("Sikemux could not start it: {error}", error = error),
        (Phase::Failed(None), true) => t!("The installer stopped. Its output above says why.").to_owned(),
        (Phase::Failed(None), false) => t!("The sign-in stopped. Its output above says why.").to_owned(),
        (Phase::Failed(Some(code)), true) => t!(
            "The installer stopped with exit code {code}. Its output above says why.",
            code = code
        ),
        (Phase::Failed(Some(code)), false) => t!(
            "The sign-in stopped with exit code {code}. Its output above says why.",
            code = code
        ),
        (Phase::Done(None), true) => t!(
            "The installer finished, but Sikemux can't find {label} yet. Open a new terminal or restart Sikemux.",
            label = label
        ),
        (Phase::Done(Some(status)), true) => t!(
            "{label} is installed: {status}.",
            label = label,
            status = status_label(status).to_lowercase()
        ),
        (Phase::Done(Some(status @ AgentStatus::Ready { .. })), false) => t!(
            "{label} is signed in: {status}.",
            label = label,
            status = status_label(status).to_lowercase()
        ),
        (Phase::Done(Some(AgentStatus::SignedOut)), false) => t!(
            "{label} still reads as signed out. Try again, or check the output above.",
            label = label
        ),
        (Phase::Done(_), false) => t!(
            "The sign-in finished. Start a {label} chat to use it.",
            label = label
        ),
    }
}

/// The running command's title, terminal, state and buttons.
fn body(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<Stateful<Div>> {
    let sheet = bench.agent_setup.sheet.as_ref()?;
    let t = Tokens::get(cx).clone();
    let title = match sheet.install {
        true => t!("Install {agent}", agent = sheet.label),
        false => t!("Sign in to {agent}", agent = sheet.label),
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
    let body = div()
        .id("agent-sheet")
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .px(SPACE_4)
                .py(SPACE_2H)
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
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
                .gap(SPACE_2)
                .px(SPACE_4)
                .py(SPACE_2H)
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(state(sheet)),
                )
                .when(failed, |foot| {
                    foot.child(button("agent-sheet-again", t!("Try again"), again))
                })
                .child(button(
                    "agent-sheet-close",
                    if running { t!("Cancel") } else { t!("Close") },
                    close,
                )),
        );
    Some(body)
}

/// Just the running command's terminal, for a page that says the rest itself.
pub fn terminal_pane(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let session = bench.agent_setup.sheet.as_ref()?.session?;
    Some(crate::terminal::pane(
        bench,
        this,
        &crate::hosts::Place::This,
        &session.to_string(),
        cx,
    ))
}

/// What takes the keys in the running command's terminal, once it has been drawn.
pub fn terminal_focus(bench: &Workbench) -> Option<FocusHandle> {
    let session = bench.agent_setup.sheet.as_ref()?.session?;
    bench
        .terminals
        .focus_of(&crate::hosts::Place::This, session)
}

pub fn sheet(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let card = body(bench, this, cx)?
        .w(px(760.))
        .h(px(520.))
        .bg(t.surface_overlay)
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_4)
        .shadow(t.shadow_float())
        .occlude();
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
