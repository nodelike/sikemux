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
use sikemux_view::chat::{
    ChatActivity, ChatConnection, FailureKind, ToolAction, ToolFamily, ToolIntent, ToolRow,
};
use sikemux_wire::protocol::AgentStatus;

use super::Chat;
use crate::agents::rows::AgentRow;
use crate::core::ChatAsk;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    Tokens, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS, TEXT_3XS_H, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, Variant};
use crate::workbench::Workbench;

/// The line under the last row while a turn runs: what the agent is doing, and for how
/// long. Each second the app's clock draws this line again in place, leaving the chat around
/// it as it was.
pub struct Activity {
    provider: String,
    /// The chat's agent as the pane last drew it, whose face the line wears.
    agent: Option<AgentRow>,
    label: Option<Doing>,
    /// The words the label had before, fading out as the new ones fade in, and since when.
    before: Option<(SharedString, Instant)>,
    since: Instant,
}

/// What the agent is doing, said as a verb and what it is on: the command it runs, the agent
/// it waits for, why it retries.
#[derive(Clone, PartialEq)]
pub struct Doing {
    verb: SharedString,
    what: Option<SharedString>,
    /// `what` is a command, set as typed.
    typed: bool,
}

impl Doing {
    fn said(verb: impl Into<SharedString>) -> Self {
        Doing {
            verb: verb.into(),
            what: None,
            typed: false,
        }
    }

    fn on(verb: impl Into<SharedString>, what: impl Into<SharedString>, typed: bool) -> Self {
        let what: SharedString = what.into();
        Doing {
            verb: verb.into(),
            what: (!what.trim().is_empty()).then_some(what),
            typed,
        }
    }

    fn whole(&self) -> SharedString {
        match &self.what {
            Some(what) => format!("{} {what}", self.verb).into(),
            None => self.verb.clone(),
        }
    }
}

impl Activity {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            agent: None,
            label: None,
            before: None,
            since: Instant::now(),
        }
    }

    pub fn wear(&mut self, agent: &AgentRow, cx: &mut Context<Self>) {
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
        let Some(label) = self.label.clone() else {
            return div().h(px(END_SPACE)).into_any_element();
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
                    .child(
                        div()
                            .flex_none()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(crate::ui::shimmer::shimmer(
                                format!("activity:{}", cx.entity_id()),
                                label.verb.clone(),
                            )),
                    )
                    .children(what),
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
                .child(match &self.agent {
                    Some(agent) => agent.mark(21., cx),
                    None => ui::mark(&self.provider, 21.),
                })
                .child(words)
                .when(seconds > 0, |line| {
                    line.child(
                        div()
                            .flex_none()
                            .child(ui::mono(crate::ui::time::elapsed(seconds), TEXT_3XS_H, cx))
                            .text_color(t.text_disabled),
                    )
                }),
        )
        .pt(SPACE_3)
        .into_any_element()
    }
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
                let verb = match (connection, attempt) {
                    (true, _) => "Reconnecting".to_owned(),
                    (false, Some(attempt)) => format!("Retrying, attempt {attempt}"),
                    (false, None) => "Retrying".to_owned(),
                };
                Doing::on(verb, reason.clone().unwrap_or_default(), false)
            }
            ChatActivity::Call { row } => doing(row),
            ChatActivity::Thinking => Doing::said("Thinking"),
            ChatActivity::Starting => Doing::said("Starting agent"),
            ChatActivity::Connecting => Doing::said("Connecting to agent session"),
        })
    }
}

/// What a running call is doing: its verb, and what it is on in its row's words.
fn doing(row: &ToolRow) -> Doing {
    use ToolFamily as F;
    use ToolIntent as I;
    let target = row.target.as_str();
    let subject = row.subject.as_deref().unwrap_or(target);
    let agent = |name: &str| {
        if name.is_empty() || name.starts_with('@') {
            name.to_owned()
        } else {
            format!("@{name}")
        }
    };
    match (row.intent, row.family) {
        (I::Run | I::Test | I::Build | I::Git | I::Commit, F::Shell) => {
            Doing::on("Running", target, true)
        }
        (I::Commit, _) => Doing::on("Committing", target, false),
        (I::Wait, F::Waiting) if row.subject.is_some() => {
            Doing::on("Waiting for", agent(subject), false)
        }
        (I::Wait, _) => Doing::on("Waiting", target, false),
        (I::Message, _) => Doing::on("Messaging", agent(subject), false),
        (I::Start, F::Agents) => Doing::on("Starting", agent(subject), false),
        (I::Start, _) => Doing::on("Starting", subject, false),
        (I::Read, F::Files | F::Shell) | (I::View, _) => Doing::on("Reading", subject, false),
        (I::Search, F::Web) => Doing::on("Searching the web for", subject, false),
        (I::Search, _) => Doing::on("Searching for", subject, true),
        (I::List, _) => Doing::on("Listing", subject, false),
        (I::Edit, _) => Doing::on("Editing", target, false),
        (I::Write, _) => Doing::on("Writing", target, false),
        (I::Open, _) => Doing::on("Opening", target, false),
        (I::Click, F::Simulator) => Doing::on("Tapping", target, false),
        (I::Click, _) => Doing::on("Clicking", target, false),
        (I::Type, _) => Doing::on("Typing", target, false),
        (I::Look, _) => Doing::said("Taking a screenshot"),
        (I::Fetch, _) => Doing::on("Reading", target, false),
        _ => match &row.activity {
            Some(activity) => Doing::said(activity.clone()),
            None => Doing::said(match row.action {
                ToolAction::Read => "Reading",
                ToolAction::Edit => "Editing",
                ToolAction::Delete => "Deleting",
                ToolAction::Move => "Moving",
                ToolAction::Search => "Searching",
                ToolAction::Execute => "Running a command",
                ToolAction::Think => "Thinking",
                ToolAction::Fetch => "Fetching",
                ToolAction::SwitchMode => "Switching mode",
                ToolAction::Mcp | ToolAction::Other => "Working",
            }),
        },
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
        action(&this, &agent.id, label, id, run)
    };
    let line = |icon: Icon, said: String| {
        div()
            .px(px(22.))
            .py(SPACE_1H)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2)
            .child(ui::icon(icon, 14.).text_color(t.danger))
            .child(
                div()
                    .min_w_0()
                    .text_size(crate::theme::ui_text(TEXT_SM))
                    .text_color(t.text_secondary)
                    .child(said),
            )
    };
    let dropped = matches!(
        head.connection,
        ChatConnection::Error | ChatConnection::Stopped
    );
    if dropped && !chat.is_empty(cx) {
        let said = head
            .ended
            .as_ref()
            .and_then(|ended| ended.message.clone())
            .map_or_else(
                || "This session dropped.".to_owned(),
                |why| format!("This session dropped: {why}"),
            );
        let row = line(Icon::Plug, said)
            .child(act("Reconnect", "chat-reconnect", |bench, agent| {
                bench.reconnect(agent)
            }))
            .child(act("Start new chat", "chat-start-over", |bench, agent| {
                bench.start_over(agent)
            }));
        return Some(centred(row).into_any_element());
    }
    let error = head.error.clone()?;
    let failure = head.failure.as_ref();
    let network = failure.is_some_and(|failure| failure.kind == FailureKind::Network);
    let said = if network {
        let served_by = sikemux_core::agent_catalog::agent(&agent.provider)
            .and_then(|agent| agent.maker)
            .unwrap_or("the model provider");
        format!("Lost connection to {served_by}. Sikemux sends it again when you're back online.",)
    } else {
        error
    };
    let mut row = line(Icon::Warning, said);
    if network {
        row = row
            .child(act("Retry now", "chat-retry", |bench, agent| {
                bench.account_call::<call::RetryFailed>(agent)
            }))
            .child(act("Discard", "chat-discard", |bench, agent| {
                bench.account_call::<call::DiscardFailed>(agent)
            }));
    }
    let resumes = failure.and_then(|failure| {
        failure
            .resumes_at
            .filter(|_| failure.kind == FailureKind::Limit)
    });
    if resumes.is_some_and(|at| at > now_ms()) {
        row = row.child(act("Don't resume", "chat-cancel-resume", |bench, agent| {
            bench.account_call::<call::CancelResume>(agent)
        }));
    }
    let kind = failure.map(|failure| failure.kind);
    if matches!(kind, Some(FailureKind::SignIn | FailureKind::Limit)) {
        row = row.children(account_ways(
            bench,
            agent,
            failure,
            &head.capabilities,
            &this,
            cx,
        ));
    }
    let details = failure
        .and_then(|failure| failure.details.clone())
        .filter(|_| network);
    let details = details.map(|details| {
        div().px(px(44.)).pb(SPACE_1H).child(
            ui::mono(details, TEXT_3XS_H, cx)
                .whitespace_normal()
                .text_color(t.text_tertiary),
        )
    });
    Some(centred(div().flex().flex_col().child(row).children(details)).into_any_element())
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
    Some(centred(row).into_any_element())
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
/// agent when the chat has no account of its own, or carry the chat on in another account the
/// core says can take it, once this one is at its limit.
fn account_ways(
    bench: &Workbench,
    agent: &AgentRow,
    failure: Option<&sikemux_view::chat::ChatFailure>,
    capabilities: &ChatCapabilities,
    this: &WeakEntity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<Button> {
    let limit = failure.is_some_and(|failure| failure.kind == FailureKind::Limit);
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
    if limit {
        let Some(on) = on else {
            return Vec::new();
        };
        let Some(takers) = crate::accounts::takers(bench, &agent.id, &on.id, cx) else {
            return Vec::new();
        };
        return (takers.others.iter())
            .filter_map(|other| bench.accounts.profile(&other.id).cloned())
            .map(|other| {
                let status = crate::accounts::status(bench, &other).and_then(|s| s.as_ref().ok());
                let name = crate::accounts::name(bench, &other, status);
                let (this, chat) = (this.clone(), agent.id.clone());
                Button::new(
                    SharedString::from(format!("chat-continue-{}", other.id)),
                    format!("Continue on {name}"),
                    Variant::Outline,
                )
                .on_click(move |_, _, cx| {
                    this.update(cx, |bench, cx| {
                        crate::accounts::move_chat(bench, &chat, &other, cx)
                    })
                    .ok();
                })
            })
            .collect();
    }
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
fn centred(line: Div) -> Div {
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(line.w_full().max_w(px(800.)))
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
        .child("Plan")
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
