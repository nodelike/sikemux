//! How a chat stands, said at the end of its rows: what the agent is doing while a turn
//! runs, why the last turn failed and what can be done about it, and a session that
//! dropped.

use sikemux_view::chat::{ChatCapabilities, PlanEntry, PlanStatus, SignInMethod};
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Context, Div, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
};
use sikemux_core::server::agent_accounts::{Authenticate, ChatAccountsParams};
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::TakeStepParams;
use sikemux_view::chat::{
    ChatActivity, ChatConnection, FailureKind, ToolAction, ToolFamily, ToolIntent, ToolRow,
};
use sikemux_view::turn::{Remedy, TurnPhase, TurnState, TurnStep};
use sikemux_wire::protocol::AgentStatus;

use super::Chat;
use crate::agents::rows::AgentRow;
use crate::core::ChatAsk;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_3XS_H, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::turn_line;
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// The line under the last row while a turn runs: what the agent is doing, and for how
/// long. Each second the app's clock draws this line again in place, leaving the chat around
/// it as it was.
pub struct Activity {
    provider: String,
    /// The core the chat's agent runs on, which takes the line's steps.
    target: crate::core::Target,
    /// The window, which says why the core refused a step.
    bench: Option<WeakEntity<Workbench>>,
    /// How the chat's turn stands, as the core last said.
    turn: Option<TurnState>,
    /// The chat's agent as the pane last drew it, whose face the line wears.
    agent: Option<AgentRow>,
    label: Option<Doing>,
    /// The words the label had before, fading out as the new ones fade in, and since when.
    before: Option<(SharedString, Instant)>,
    since: Instant,
}

/// What the agent is doing, said as a verb and what it is on: the command it runs, the agent
/// it waits for, why it retries. A language puts the verb before or after what it is on, so the
/// words come from a whole sentence with `{what}` in it, split where `what` goes.
#[derive(Clone, PartialEq)]
pub struct Doing {
    verb: SharedString,
    /// The words after `what`, in a language that says them there.
    after: Option<SharedString>,
    what: Option<SharedString>,
    /// `what` is a command, set as typed.
    typed: bool,
}

impl Doing {
    fn said(verb: impl Into<SharedString>) -> Self {
        Doing {
            verb: verb.into(),
            after: None,
            what: None,
            typed: false,
        }
    }

    fn on(sentence: &str, what: impl Into<SharedString>, typed: bool) -> Self {
        let what: SharedString = what.into();
        let (before, after) = sentence.split_once("{what}").unwrap_or((sentence, ""));
        let (before, after) = (before.trim(), after.trim());
        Doing {
            verb: before.to_owned().into(),
            after: (!after.is_empty()).then(|| after.to_owned().into()),
            what: Some(what),
            typed,
        }
    }

    /// Only the verb, when what it is on already shows above it.
    pub fn without_what(self) -> Self {
        let verb = match &self.after {
            Some(after) => format!("{} {after}", self.verb).trim().to_owned().into(),
            None => self.verb,
        };
        Doing::said(verb)
    }

    pub(super) fn whole(&self) -> SharedString {
        let parts = [Some(&self.verb), self.what.as_ref(), self.after.as_ref()];
        let words: Vec<&str> = parts
            .into_iter()
            .flatten()
            .map(|part| part.as_ref())
            .collect();
        words.join(" ").trim().to_owned().into()
    }
}

impl Activity {
    pub fn new(provider: &str, target: crate::core::Target) -> Self {
        Self {
            provider: provider.to_owned(),
            target,
            bench: None,
            turn: None,
            agent: None,
            label: None,
            before: None,
            since: Instant::now(),
        }
    }

    pub fn wear(&mut self, agent: &AgentRow, bench: WeakEntity<Workbench>, cx: &mut Context<Self>) {
        self.bench.get_or_insert(bench);
        let worn = self.agent.as_ref().is_some_and(|worn| {
            worn.id == agent.id
                && worn.provider == agent.provider
                && worn.face == agent.face
                && worn.group == agent.group
        });
        if !worn {
            self.agent = Some(agent.clone());
            cx.notify();
        }
    }

    pub fn set_turn(&mut self, turn: Option<&TurnState>, cx: &mut Context<Self>) {
        if self.turn.as_ref() != turn {
            self.turn = turn.cloned();
            cx.notify();
        }
    }

    fn mark(&self, cx: &App) -> AnyElement {
        match &self.agent {
            Some(agent) => agent.mark(21., cx),
            None => ui::mark(&self.provider, 21.).into_any_element(),
        }
    }

    /// The line once the turn has ended, or while it works on a step it named: how it stands,
    /// said by the core, with the next steps as buttons. A failed turn's line is `trouble`'s.
    fn state_line(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let turn = self.turn.as_ref()?;
        if turn.state == TurnPhase::Failed {
            return None;
        }
        let said = turn_line::said(turn, turn_line::maker(&self.provider), ui::time::now_ms())?;
        if turn_line::ticks(turn.state) {
            ui::clock::second(window, cx);
        }
        let take = |step: TurnStep, variant: Variant| {
            let label = turn_line::step_label(step)?;
            let agent = self.agent.as_ref()?.id.clone();
            let (target, bench) = (self.target.clone(), self.bench.clone()?);
            Some(
                step_button(label, variant)
                    .on_click(move |_, _, cx| {
                        let taken = target.request_typed::<call::TakeStep>(TakeStepParams {
                            agent_id: agent.clone(),
                            step,
                        });
                        let bench = bench.clone();
                        cx.spawn(async move |cx| {
                            if let Err(refused) = taken.await {
                                cx.update(|cx| {
                                    crate::toast::show(&bench, crate::toast::error(refused), cx)
                                });
                            }
                        })
                        .detach();
                    })
                    .into_any_element(),
            )
        };
        let buttons = match turn.state {
            TurnPhase::Done => (turn.done.iter())
                .flat_map(|done| &done.next)
                .filter_map(|next| {
                    let variant = match next.primary {
                        true => Variant::Primary,
                        false => Variant::Outline,
                    };
                    take(next.step, variant)
                })
                .collect(),
            TurnPhase::NeedsYou => self.look().into_iter().collect(),
            TurnPhase::Stopped => take(TurnStep::CarryOn, Variant::Outline)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        let key = SharedString::from(format!("turn:{}", cx.entity_id()));
        Some(
            centred(
                turn_line::line(self.mark(cx), &said, key, buttons, cx).px(px(22.)),
                cx,
            )
            .pt(SPACE_3)
            .pb(px(END_SPACE))
            .into_any_element(),
        )
    }

    /// Look: the chat's end, where the call it waits on sits, with the keys on its card.
    fn look(&self) -> Option<AnyElement> {
        let agent = self.agent.as_ref()?.id.clone();
        let bench = self.bench.clone()?;
        Some(
            step_button("Look", Variant::Primary)
                .on_click(move |_, window, cx| {
                    bench
                        .update(cx, |bench, cx| {
                            bench.show_latest(&agent, cx);
                            bench.focus_draft(&agent, window, cx);
                        })
                        .ok();
                })
                .into_any_element(),
        )
    }

    pub fn set(&mut self, label: Option<Doing>, cx: &mut Context<Self>) {
        if label == self.label {
            return;
        }
        if self.label.is_none() {
            self.since = Instant::now();
        }
        self.before = self
            .label
            .take()
            .map(|before| (before.whole(), Instant::now()));
        self.label = label;
        cx.notify();
    }
}

impl Render for Activity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let named_step = (self.turn.as_ref()).is_some_and(|turn| turn.on.is_some());
        let Some(label) = self.label.clone().filter(|_| !named_step) else {
            return self
                .state_line(window, cx)
                .unwrap_or_else(|| div().h(px(END_SPACE)).into_any_element());
        };
        ui::clock::second(window, cx);
        let t = Tokens::get(cx).clone();
        let seconds = self.since.elapsed().as_secs();
        let swap = motion::TEXT_SWAP;
        let through = self
            .before
            .as_ref()
            .map(|(_, at)| (at.elapsed().as_secs_f32() / swap.duration.as_secs_f32()).min(1.))
            .filter(|through| *through < 1. && !cx.reduce_motion());
        if through.is_some() {
            window.request_animation_frame();
        }
        let shown = through.map_or(1., |through| swap.at(through));
        let what = label.what.clone().map(|what| {
            let what = div()
                .min_w_0()
                .truncate()
                .text_color(t.text_secondary)
                .child(what);
            if label.typed {
                what.font_family(crate::theme::MONO)
                    .text_size(crate::theme::ui_text(TEXT_XS))
            } else {
                what.text_size(crate::theme::ui_text(TEXT_XS_H))
            }
        });
        let words = div()
            .relative()
            .min_w_0()
            .text_size(crate::theme::ui_text(TEXT_SM))
            .child(
                div()
                    .opacity(shown)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .min_w_0()
                    .when(!label.verb.is_empty(), |line| {
                        line.child(
                            div()
                                .flex_none()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(crate::ui::shimmer::shimmer(
                                    format!("activity:{}", cx.entity_id()),
                                    label.verb.clone(),
                                )),
                        )
                    })
                    .children(what)
                    .children(label.after.clone().map(|after| {
                        div()
                            .flex_none()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(crate::ui::shimmer::shimmer(
                                format!("activity-after:{}", cx.entity_id()),
                                after,
                            ))
                    })),
            )
            .children(through.and(self.before.as_ref()).map(|(before, _)| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .truncate()
                    .opacity(1. - shown)
                    .text_color(t.text_disabled)
                    .child(before.clone())
            }));
        centred(
            div()
                .px(px(22.))
                .pb(px(30.))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(self.mark(cx))
                .child(words)
                .when(seconds > 0, |line| {
                    line.child(
                        div()
                            .flex_none()
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .text_color(t.text_tertiary)
                            .child(crate::ui::time::elapsed(seconds)),
                    )
                }),
            cx,
        )
        .pt(SPACE_3)
        .into_any_element()
    }
}

fn step_button(label: &'static str, variant: Variant) -> Button {
    Button::new(
        SharedString::from(format!("turn-step-{label}")),
        label,
        variant,
    )
    .sized(26., 10., TEXT_XS)
}

/// The room after the chat's last row, kept here rather than on the row so a row arriving
/// does not take it from the one before.
const END_SPACE: f32 = 27.;

impl Chat {
    /// What the line under the last row says: nothing while a permission card already says
    /// what the turn waits on, else the running call's words, a retry, that it thinks,
    /// or that the agent is still on its way to a message sent to it.
    pub(super) fn activity(&self) -> Option<Doing> {
        let head = self.state.as_ref()?;
        Some(match head.activity.as_ref()? {
            ChatActivity::Retrying {
                attempt,
                connection,
                reason,
            } => {
                let why = reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|why| !why.is_empty());
                let Some(why) = why else {
                    return Some(Doing::said(match (connection, attempt) {
                        (true, _) => t!("Reconnecting").to_owned(),
                        (false, Some(attempt)) => {
                            t!("Retrying, attempt {attempt}", attempt = attempt)
                        }
                        (false, None) => t!("Retrying").to_owned(),
                    }));
                };
                let sentence = match (connection, attempt) {
                    (true, _) => t!("Reconnecting: {what}").to_owned(),
                    (false, Some(attempt)) => {
                        t!("Retrying, attempt {attempt}: {what}", attempt = attempt)
                    }
                    (false, None) => t!("Retrying: {what}").to_owned(),
                };
                Doing::on(&sentence, why.to_owned(), false)
            }
            ChatActivity::Call { row } => doing(row),
            ChatActivity::Thinking => Doing::said(t!("Thinking")),
            ChatActivity::Starting => Doing::said(t!("Starting agent")),
            ChatActivity::Connecting => Doing::said(t!("Connecting to agent session")),
        })
    }
}

/// What a running call is doing: its verb, and what it is on in its row's words.
fn doing(row: &ToolRow) -> Doing {
    use ToolFamily as F;
    use ToolIntent as I;
    let target = row.target.as_str();
    let subject = row.subject.as_deref().unwrap_or(target);
    if subject.trim().is_empty() {
        return said_action(row);
    }
    let agent = |name: &str| {
        if name.is_empty() || name.starts_with('@') {
            name.to_owned()
        } else {
            format!("@{name}")
        }
    };
    match (row.intent, row.family) {
        (I::Run | I::Test | I::Build | I::Git | I::Commit, F::Shell) => {
            Doing::on(t!("Running {what}"), target, true)
        }
        (I::Commit, _) => Doing::on(t!("Committing {what}"), target, false),
        (I::Wait, F::Waiting) if row.subject.is_some() => {
            Doing::on(t!("Waiting for {what}"), agent(subject), false)
        }
        (I::Wait, _) => Doing::on(t!("Waiting {what}"), target, false),
        (I::Message, _) => Doing::on(t!("Messaging {what}"), agent(subject), false),
        (I::Start, F::Agents) => Doing::on(t!("Starting {what}"), agent(subject), false),
        (I::Start, _) => Doing::on(t!("Starting {what}"), subject, false),
        (I::Read, F::Files | F::Shell) | (I::View, _) => {
            Doing::on(t!("Reading {what}"), subject, false)
        }
        (I::Search, F::Web) => Doing::on(t!("Searching the web for {what}"), subject, false),
        (I::Search, _) => Doing::on(t!("Searching for {what}"), subject, true),
        (I::List, _) => Doing::on(t!("Listing {what}"), subject, false),
        (I::Edit, _) => Doing::on(t!("Editing {what}"), target, false),
        (I::Write, _) => Doing::on(t!("Writing {what}"), target, false),
        (I::Open, _) => Doing::on(t!("Opening {what}"), target, false),
        (I::Click, F::Simulator) => Doing::on(t!("Tapping {what}"), target, false),
        (I::Click, _) => Doing::on(t!("Clicking {what}"), target, false),
        (I::Type, _) => Doing::on(t!("Typing {what}"), target, false),
        (I::Look, _) => Doing::said(t!("Taking a screenshot")),
        (I::Fetch, _) => Doing::on(t!("Reading {what}"), target, false),
        _ => said_action(row),
    }
}

/// A call's own word for what it does, or its kind's, when it says nothing it is on.
fn said_action(row: &ToolRow) -> Doing {
    match &row.activity {
        Some(activity) => Doing::said(activity.clone()),
        None => Doing::said(match row.action {
            ToolAction::Read => t!("Reading"),
            ToolAction::Edit => t!("Editing"),
            ToolAction::Delete => t!("Deleting"),
            ToolAction::Move => t!("Moving"),
            ToolAction::Search => t!("Searching"),
            ToolAction::Execute => t!("Running a command"),
            ToolAction::Think => t!("Thinking"),
            ToolAction::Fetch => t!("Fetching"),
            ToolAction::SwitchMode => t!("Switching mode"),
            ToolAction::Mcp | ToolAction::Other => t!("Working"),
        }),
    }
}

impl Workbench {
    /// Starts the session of `agent`'s chat again after it dropped, and attaches to it.
    pub fn reconnect(&mut self, agent: &str) {
        self.ask_chat(ChatAsk::Restart {
            agent: agent.to_owned(),
        });
    }

    /// A new chat with the same agent and account, in the same project.
    fn start_over(&mut self, agent: &str) {
        let Some(saved) = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
        else {
            return;
        };
        let launcher = self.launchers().iter().find(|launcher| {
            launcher.provider == saved.provider
                && saved
                    .profile_id
                    .as_ref()
                    .is_none_or(|profile| launcher.id.ends_with(&format!(":{profile}")))
        });
        if let Some(launcher) = launcher {
            self.chat_target(agent).start_chat(
                launcher.id.clone(),
                saved.project.clone(),
                None,
                None,
            );
        }
    }

    /// Signs the chat's agent in the way it offered, then sends its failed turn again.
    fn authenticate_chat(&self, agent: &str, method: &str) {
        let target = self.chat_target(agent);
        let signed = target.request_typed::<call::AuthenticateChat>(Authenticate {
            agent_id: agent.to_owned(),
            method_id: method.to_owned(),
        });
        let agent = agent.to_owned();
        crate::desk::spawn(async move {
            match signed.await {
                Ok(_) => {
                    target.call_typed::<call::RetryFailed>(ChatAccountsParams { agent_id: agent })
                }
                Err(error) => eprintln!("sikemux: the agent did not sign in: {error}"),
            }
        });
    }

    /// Runs the agent's own sign-in in a terminal sheet when it has one, else opens Settings
    /// to add an account for it.
    fn sign_in_agent(&mut self, provider: &str, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = self
            .agent_setup
            .found
            .iter()
            .flatten()
            .find(|found| found.kind == provider)
            .and_then(|found| crate::agents::setup::signing_in(found, None));
        match sheet {
            Some(sheet) => crate::agents::setup::run(self, sheet, cx),
            None => crate::accounts::open_sign_in(self, provider, None, window, cx),
        }
    }

    fn account_call<C: Call<Params = ChatAccountsParams>>(&self, agent: &str) {
        self.chat_target(agent).call_typed::<C>(ChatAccountsParams {
            agent_id: agent.to_owned(),
        });
    }
}

/// What stands at the end of the rows when something went wrong: why the last turn
/// failed with what can be done about it, or that the session dropped.
pub(super) fn trouble(
    bench: &Workbench,
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let head = chat.state.as_ref()?;
    let t = Tokens::get(cx).clone();
    let this = cx.weak_entity();
    let act = |label: &'static str, id: &'static str, run: fn(&mut Workbench, &str)| {
        action(&this, &agent.id, label, id, run).into_any_element()
    };
    let key = SharedString::from(format!("turn-failed:{}", agent.id));
    let failed = |parts: Vec<String>, buttons: Vec<AnyElement>, cx: &mut Context<Workbench>| {
        let said = turn_line::Said {
            word: "Failed".into(),
            phase: TurnPhase::Failed,
            parts: parts.into_iter().map(SharedString::from).collect(),
        };
        turn_line::line(agent.mark(21., cx), &said, key.clone(), buttons, cx)
            .flex_wrap()
            .px(px(22.))
            .py(SPACE_1H)
    };
    let dropped = matches!(
        head.connection,
        ChatConnection::Error | ChatConnection::Stopped
    );
    if dropped && !chat.is_empty(cx) {
        let why = (head.ended.as_ref()).and_then(|ended| ended.message.clone());
        let mut parts = vec!["the session dropped".to_owned()];
        parts.extend(why);
        let buttons = vec![
            act("Reconnect", "chat-reconnect", |bench, agent| {
                bench.reconnect(agent)
            }),
            act("Start new chat", "chat-start-over", |bench, agent| {
                bench.start_over(agent)
            }),
        ];
        return Some(centred(failed(parts, buttons, cx), cx).into_any_element());
    }
    let error = head.error.clone()?;
    let failure = head.failure.as_ref();
    let network = failure.is_some_and(|failure| failure.kind == FailureKind::Network);
    let maker = turn_line::maker(&agent.provider);
    let parts = match head
        .turn
        .as_ref()
        .filter(|turn| turn.state == TurnPhase::Failed)
    {
        Some(turn) => turn_line::said(turn, maker, now_ms())
            .map(|said| said.parts.into_iter().map(String::from).collect())
            .unwrap_or_else(|| vec![error.clone()]),
        None if network => vec![format!("lost the connection to {maker}")],
        None => vec![error.clone()],
    };
    let mut buttons = Vec::new();
    if network {
        buttons.push(act("Retry now", "chat-retry", |bench, agent| {
            bench.account_call::<call::RetryFailed>(agent)
        }));
        buttons.push(act("Discard", "chat-discard", |bench, agent| {
            bench.account_call::<call::DiscardFailed>(agent)
        }));
    }
    let kind = failure.map(|failure| failure.kind);
    if kind == Some(FailureKind::Limit) {
        let can = (head.turn.as_ref())
            .and_then(|turn| turn.failed.as_ref())
            .map(|failed| failed.can.as_slice())
            .unwrap_or_default();
        if can.contains(&Remedy::OtherAccount) {
            buttons.extend(other_account(bench, agent, failure, &this, cx));
        }
        let carry_on_with = failure.and_then(|failure| failure.carry_on_with.clone());
        if let Some(other) = carry_on_with {
            let (this, chat) = (this.clone(), agent.id.clone());
            let label = format!("Hand to {}", bench.agent_name(&other));
            let button =
                Button::new("chat-hand-off", label, Variant::Outline).on_click(move |_, _, cx| {
                    let other = other.clone();
                    this.update(cx, |bench, cx| {
                        let to = crate::chat::model_picker::Moving {
                            profile: bench
                                .accounts
                                .current(&other)
                                .map(|profile| profile.id.clone()),
                            provider: other,
                            model: None,
                            model_provider: None,
                        };
                        bench.hand_off(&chat, to, false, cx);
                    })
                    .ok();
                });
            buttons.push(button.into_any_element());
        }
        let resumes = failure
            .and_then(|failure| failure.resumes_at)
            .filter(|at| *at > now_ms());
        buttons.push(match resumes {
            Some(at) => quiet(
                &this,
                &agent.id,
                format!("Retry at {}", turn_line::moment(at)),
                |bench, agent| bench.account_call::<call::CancelResume>(agent),
            )
            .tooltip(t!(
                "Sikemux sends your last message again then. Click to not send it."
            ))
            .into_any_element(),
            None => quiet(&this, &agent.id, "Retry".into(), |bench, agent| {
                bench.account_call::<call::RetryFailed>(agent)
            })
            .into_any_element(),
        });
    }
    if kind == Some(FailureKind::SignIn) {
        buttons.extend(
            account_ways(bench, agent, failure, &head.capabilities, &this)
                .into_iter()
                .map(IntoElement::into_any_element),
        );
    }
    let details = failure
        .and_then(|failure| failure.details.clone())
        .filter(|_| network);
    let details = details.map(|details| {
        div().pl(px(51.)).pr(px(22.)).pb(SPACE_1H).child(
            ui::mono(details, TEXT_3XS_H, cx)
                .whitespace_normal()
                .text_color(t.text_tertiary),
        )
    });
    let row = failed(parts, buttons, cx);
    Some(centred(div().flex().flex_col().child(row).children(details), cx).into_any_element())
}

/// What an empty chat says while its session is not ready, in the welcome's place: that it
/// stopped or failed with a way back, or that a message waits for its agent to start. A
/// chat taken up again stays blank while its agent warms up unseen; None while the welcome
/// belongs there.
/// What `agent`'s empty chat says in place of the welcome while its session is not ready.
pub(crate) fn connection_note(
    agent: &AgentRow,
    chat: &Chat,
    resumes: bool,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let connection = chat
        .state
        .as_ref()
        .map_or(ChatConnection::Connecting, |head| head.connection);
    let dropped = matches!(connection, ChatConnection::Error | ChatConnection::Stopped);
    if connection == ChatConnection::Ready || (!resumes && !dropped && !chat.awaits_agent) {
        return None;
    }
    if !dropped && !chat.awaits_agent {
        return Some(div().into_any_element());
    }
    let t = Tokens::get(cx).clone();
    let (icon, said) = match connection {
        ChatConnection::Starting => (Icon::Hourglass, "Starting agent…"),
        ChatConnection::Error => (Icon::Plug, "Structured session unavailable."),
        ChatConnection::Stopped => (Icon::Plug, "Agent session stopped."),
        _ => (Icon::Hourglass, "Connecting to agent session…"),
    };
    let this = cx.weak_entity();
    let row = div()
        .px(px(22.))
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(SPACE_2)
        .child(ui::icon(icon, 14.).text_color(t.text_tertiary))
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_SM))
                .text_color(t.text_secondary)
                .child(said),
        )
        .when(dropped, |row| {
            row.child(action(
                &this,
                &agent.id,
                "Reconnect",
                "chat-reconnect",
                |bench, agent| bench.reconnect(agent),
            ))
            .child(action(
                &this,
                &agent.id,
                "Start new chat",
                "chat-start-over",
                |bench, agent| bench.start_over(agent),
            ))
        });
    Some(centred(row, cx).into_any_element())
}

/// A quiet button that says what Sikemux will do on its own, such as when it retries.
fn quiet(
    this: &WeakEntity<Workbench>,
    agent: &str,
    label: String,
    run: fn(&mut Workbench, &str),
) -> Button {
    let (this, agent) = (this.clone(), agent.to_owned());
    Button::new(
        SharedString::from(format!("chat-quiet-{agent}")),
        label,
        Variant::Ghost,
    )
    .on_click(move |_, _, cx| {
        this.update(cx, |bench, cx| {
            run(bench, &agent);
            cx.notify();
        })
        .ok();
    })
}

/// "Use another account": the accounts the core says can take the chat on, once the one it
/// runs on is at its limit, to move it to as the chat's account control does.
fn other_account(
    bench: &Workbench,
    agent: &AgentRow,
    failure: Option<&sikemux_view::chat::ChatFailure>,
    this: &WeakEntity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    if !matches!(
        bench.chat_target(&agent.id).place(),
        crate::hosts::Place::This
    ) {
        return None;
    }
    let saved = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent.id)
    });
    let on = failure
        .and_then(|failure| failure.account.as_deref())
        .or_else(|| saved.and_then(|saved| saved.profile_id.as_deref()))
        .and_then(|id| bench.accounts.profile(id))
        .or_else(|| bench.accounts.current(&agent.provider))
        .cloned()?;
    let takers = crate::accounts::takers(bench, &agent.id, &on.id, cx)?;
    let others: Vec<(sikemux_core::agent_accounts::Profile, String)> = (takers.others.iter())
        .filter_map(|other| bench.accounts.profile(&other.id).cloned())
        .map(|other| {
            let status = crate::accounts::status(bench, &other).and_then(|s| s.as_ref().ok());
            let name = other.shown_name(status);
            (other, name)
        })
        .collect();
    if others.is_empty() {
        return None;
    }
    let (this, chat) = (this.clone(), agent.id.clone());
    let trigger = Button::new(
        "chat-other-account",
        t!("Use another account"),
        Variant::Outline,
    );
    Some(
        ui::menu_button(
            "chat-other-account-menu",
            gpui_kit::Anchor::BottomRight,
            trigger,
            move |mut popup, _, cx| {
                for (other, name) in &others {
                    let (this, chat, other) = (this.clone(), chat.clone(), other.clone());
                    popup = popup.item(ui::MenuRow::new(name.clone()).item(cx).on_click(
                        move |_, _, cx| {
                            this.update(cx, |bench, cx| {
                                crate::accounts::move_chat(bench, &chat, &other, cx)
                            })
                            .ok();
                        },
                    ));
                }
                popup
            },
        )
        .into_any_element(),
    )
}

fn action(
    this: &WeakEntity<Workbench>,
    agent: &str,
    label: &'static str,
    id: &'static str,
    run: fn(&mut Workbench, &str),
) -> Button {
    let (this, agent) = (this.clone(), agent.to_owned());
    Button::new(id, label, Variant::Outline).on_click(move |_, _, cx| {
        this.update(cx, |bench, cx| {
            run(bench, &agent);
            cx.notify();
        })
        .ok();
    })
}

/// What the account the chat runs on can do about a turn it failed: sign in to it, or to the
/// agent when the chat has no account of its own.
fn account_ways(
    bench: &Workbench,
    agent: &AgentRow,
    failure: Option<&sikemux_view::chat::ChatFailure>,
    capabilities: &ChatCapabilities,
    this: &WeakEntity<Workbench>,
) -> Vec<Button> {
    // The accounts listed are this host's, and moving a chat to one is this host's to do.
    if !matches!(
        bench.chat_target(&agent.id).place(),
        crate::hosts::Place::This
    ) {
        return Vec::new();
    }
    let saved = bench.shown().and_then(|view| {
        view.workspace
            .agents
            .iter()
            .find(|saved| saved.id == agent.id)
    });
    let on = failure
        .and_then(|failure| failure.account.as_deref())
        .or_else(|| saved.and_then(|saved| saved.profile_id.as_deref()))
        .and_then(|id| bench.accounts.profile(id))
        .or_else(|| bench.accounts.current(&agent.provider))
        .cloned();
    let name = match &on {
        Some(profile) => profile.name.clone(),
        None => bench
            .launchers()
            .iter()
            .find(|launcher| launcher.provider == agent.provider)
            .map_or_else(|| agent.provider.clone(), |launcher| launcher.label.clone()),
    };
    let retry = action(
        this,
        &agent.id,
        "Retry",
        "chat-retry-signed-in",
        |bench, agent| bench.account_call::<call::RetryFailed>(agent),
    );
    if on.is_none() {
        let methods: Vec<Button> = (capabilities.sign_in_methods.iter())
            .filter_map(|method| {
                let SignInMethod::Agent {
                    id, name: label, ..
                } = method
                else {
                    return None;
                };
                let (this, chat, method) = (this.clone(), agent.id.clone(), id.to_owned());
                Some(
                    Button::new(
                        SharedString::from(format!("chat-sign-in-{id}")),
                        label.to_owned(),
                        Variant::Outline,
                    )
                    .on_click(move |_, _, cx| {
                        this.update(cx, |bench, _| bench.authenticate_chat(&chat, &method))
                            .ok();
                    }),
                )
            })
            .collect();
        if !methods.is_empty() {
            return methods;
        }
    }
    let moves_on_sign_in = on.is_some();
    let (this, provider, account, chat) = (
        this.clone(),
        agent.provider.clone(),
        on.map(|on| on.id),
        agent.id.clone(),
    );
    let sign_in = Button::new(
        "chat-sign-in",
        format!("Sign in to {name}"),
        Variant::Outline,
    )
    .on_click(move |_, window, cx| {
        this.update(cx, |bench, cx| match &account {
            Some(account) => crate::accounts::sign_in_then_move(bench, &chat, account, window, cx),
            None => bench.sign_in_agent(&provider, window, cx),
        })
        .ok();
    });
    match moves_on_sign_in {
        true => vec![sign_in],
        false => vec![sign_in, retry],
    }
}

/// A line at the end of the rows, in the column the rows are read in.
fn centred(line: Div, cx: &App) -> Div {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(line.w_full().max_w(super::column::rows(cx)))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// The plan the agent keeps for the turn, folded behind its name until the reader opens it.
pub(super) fn plan(
    agent: &AgentRow,
    chat: &Chat,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let plan = chat.state.as_ref()?.plan.as_ref()?;
    let t = Tokens::get(cx).clone();
    let open = chat.plan_open;
    let this = cx.weak_entity();
    let agent = agent.id.clone();
    let done = (plan.iter())
        .filter(|entry| entry.status == PlanStatus::Completed)
        .count();
    let summary = div()
        .id("chat-plan")
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(t.text_disabled)
        .cursor_pointer()
        .child(motion::turn(
            format!("plan-chevron:{agent}"),
            motion::fold_angle(open),
            motion::DETAIL_FOLD,
            ui::icon(Icon::Chevron, 10.).text_color(t.text_disabled),
        ))
        .child(t!("Plan"))
        .child(format!("{done} of {}", plan.len()))
        .on_click({
            let agent = agent.clone();
            move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    if let Some(chat) = bench.chats.open.get_mut(&agent) {
                        chat.plan_open = !chat.plan_open;
                    }
                    bench.parts.redraw_stage(cx);
                })
                .ok();
            }
        });
    let shown = open.then(|| {
        div()
            .id("chat-plan-steps")
            .mt(SPACE_2)
            .max_h(px(420.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .children(
                plan.iter()
                    .enumerate()
                    .map(|(index, entry)| plan_step(&agent, index, entry, cx)),
            )
    });
    Some(
        centred(
            div()
                .px(px(22.))
                .py(SPACE_1H)
                .flex()
                .flex_col()
                .child(summary)
                .children(shown),
            cx,
        )
        .into_any_element(),
    )
}

/// One step of a plan, with a mark for how it stands: ticked when done, working while under way,
/// an empty ring until then.
fn plan_step(agent: &str, index: usize, entry: &PlanEntry, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let mark = match entry.status {
        PlanStatus::Completed => ui::icon(Icon::Check, 12.)
            .text_color(t.text_tertiary)
            .into_any_element(),
        PlanStatus::InProgress => {
            ui::status_mark(&format!("plan:{agent}:{index}"), AgentStatus::Working, cx)
                .unwrap_or_else(|| div().into_any_element())
        }
        PlanStatus::Pending | PlanStatus::Unknown => div()
            .size(px(7.))
            .rounded_full()
            .border_1()
            .border_color(t.text_disabled)
            .into_any_element(),
    };
    let ink = match entry.status {
        PlanStatus::Completed => t.text_tertiary,
        PlanStatus::InProgress => t.text_primary,
        PlanStatus::Pending | PlanStatus::Unknown => t.text_secondary,
    };
    div()
        .flex()
        .items_start()
        .gap(SPACE_2)
        .child(ui::mark_box(16., mark))
        .child(
            div()
                .min_w_0()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(px(16.))
                .text_color(ink)
                .child(entry.content.clone()),
        )
        .into_any_element()
}
