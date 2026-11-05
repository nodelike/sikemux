//! What an agent spoken to in JSON-RPC does whichever agent it is, Codex or
//! Muse: reading its messages, carrying out what each does to the chat, and
//! sending, steering, stopping and handing over its turns. Each agent says only
//! what differs, through [`RpcSession`] and [`RpcAgent`].

use std::future::Future;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_fold::update::turn_answer;
use sikemux_process::sync::Locked;
use sikemux_view::chat::StopReason;

use crate::server::chat::failure::Failure;

use super::super::states::{Outcome, Subagent};
use super::super::Respond;
use super::pending::{Answered, Purpose};
use super::process::{self, Lines, Process, Reading, Reads, Released, Writer};
use super::rpc::{error_text, Incoming, Rpc, RpcState, Sent, ANSWER_WAIT};
use super::session::{Prompt, Sink, TurnEnd};
use super::turns::{self, Ended, Prompted, Turns};

/// What one message from the agent does to the chat. `Own` is what only this
/// agent does.
pub(super) enum Effect<Own> {
    /// A session update, for the session with this id.
    Update(String, Value),
    End(u64, TurnEnd),
    WorkStarted,
    WorkEnded(StopReason),
    /// Ask the agent to stop the turn with this id.
    Interrupt(String),
    Own(Own),
}

/// How an agent's session follows its turns. Everything here but the required
/// items is the same for every agent.
pub(super) trait RpcSession {
    type Own;
    const NAME: &'static str;

    fn id(&self) -> &str;
    fn turns(&mut self) -> &mut Turns;
    fn finish_subagents(&mut self, state: Subagent) -> Vec<Effect<Self::Own>>;

    /// What a turn's end tells the chat, unless the agent says more.
    fn payload(&self) -> Value {
        turn_answer(StopReason::EndTurn)
    }

    /// The agent answered turn `number` with its id for it.
    fn started(&mut self, number: u64, id: &str) -> Vec<Effect<Self::Own>> {
        match self.turns().started(number, id) {
            Some(true) => vec![Effect::Interrupt(id.to_owned())],
            _ => Vec::new(),
        }
    }

    /// The person asked to stop.
    fn cancel(&mut self) -> Vec<Effect<Self::Own>> {
        self.turns()
            .cancel()
            .map(Effect::Interrupt)
            .into_iter()
            .collect()
    }

    /// The agent took the interrupt of turn `id`. The turn is over for the
    /// chat now, and what the agent still says of it is dropped.
    fn interrupted(&mut self, id: &str) -> Vec<Effect<Self::Own>> {
        match self.turns().interrupted(id) {
            Ended::Prompted(turn) => {
                let payload = self.payload();
                self.end_prompted(turn, Outcome::Stopped, payload, || {
                    Failure::other("Turn failed")
                })
            }
            Ended::Unprompted => self.end_unprompted(Outcome::Stopped),
            Ended::Neither => Vec::new(),
        }
    }

    /// The prompted `turn` ended as `outcome`, with `payload` as its answer.
    fn end_prompted(
        &mut self,
        mut turn: Prompted,
        outcome: Outcome,
        payload: Value,
        failure: impl FnOnce() -> Failure,
    ) -> Vec<Effect<Self::Own>> {
        let mut effects: Vec<_> = self
            .turns()
            .title(&mut turn)
            .map(|title| Effect::Update(self.id().to_owned(), title))
            .into_iter()
            .collect();
        let (subagents, end) = turns::prompted_end(outcome, payload, failure);
        if let Some(state) = subagents {
            effects.extend(self.finish_subagents(state));
        }
        effects.push(Effect::End(turn.number, end));
        effects
    }

    /// A turn the agent began on its own ended as `outcome`.
    fn end_unprompted(&mut self, outcome: Outcome) -> Vec<Effect<Self::Own>> {
        let (subagents, reason) = turns::unprompted_end(outcome);
        let mut effects = subagents
            .map(|state| self.finish_subagents(state))
            .unwrap_or_default();
        effects.push(Effect::WorkEnded(reason));
        effects
    }

    /// A turn whose end the agent sent unreadably ends for the chat as failed,
    /// rather than leaving it waiting on an end it cannot read.
    fn ended_unreadably(&mut self, error: String) -> Vec<Effect<Self::Own>> {
        let Some(turn) = self.turns().give_up() else {
            return Vec::new();
        };
        let payload = self.payload();
        self.end_prompted(turn, Outcome::Failed, payload, || Failure {
            details: Some(error),
            ..Failure::other(format!(
                "{} ended the turn in a way Sikemux could not read",
                Self::NAME
            ))
        })
    }
}

/// What an agent says differently on the wire.
pub(super) trait RpcAgent:
    RpcSession + Default + Serialize + DeserializeOwned + Send + Sized + 'static
{
    /// Words in the agent's refusal of a steer that mean the turn had already
    /// ended. It has no error code for that.
    const TURN_ENDED: &'static [&'static str];

    /// The fields every request about session `id` carries.
    fn addressed(id: &str) -> Value;

    /// The agent's id for a turn, from its answer to the request that began it.
    fn turn_id_in(answer: &Value) -> Option<&str>;

    fn carry_out(driver: &Driver<Self>, own: Self::Own);
    fn notification(driver: &Driver<Self>, method: &str, params: Value);
    fn request(driver: &Driver<Self>, id: Value, method: &str, params: &Value);

    /// The answer to a request sent with a `purpose` only this agent has.
    fn purpose_answered(driver: &Driver<Self>, purpose: Purpose, result: Answered);

    /// Tells the agent what the person chose for the question `reply` holds.
    fn decide(driver: &Driver<Self>, reply: &Value, chosen: Option<String>);
}

/// What the reader and the chat's loop share.
pub(super) struct Driver<S> {
    pub rpc: Rpc,
    pub sink: Sink,
    pub session: Arc<Mutex<S>>,
}

impl<S> Clone for Driver<S> {
    fn clone(&self) -> Self {
        Self {
            rpc: self.rpc.clone(),
            sink: self.sink.clone(),
            session: self.session.clone(),
        }
    }
}

/// What an agent's chat hands a core that takes it over: the connection, the
/// session, and what `Kept` the chat holds beside them.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handed<S, Kept> {
    rpc: RpcState,
    session: S,
    #[serde(flatten)]
    kept: Kept,
}

impl<S: RpcAgent> Driver<S> {
    pub fn new(rpc: Rpc, sink: Sink, session: S) -> Self {
        Self {
            rpc,
            sink,
            session: Arc::new(Mutex::new(session)),
        }
    }

    pub fn apply(&self, effects: Vec<Effect<S::Own>>) {
        for effect in effects {
            match effect {
                Effect::Update(session, update) => self.sink.update(&session, update),
                Effect::End(turn, end) => self.sink.end_turn(turn, end),
                Effect::WorkStarted => self.sink.work_started(),
                Effect::WorkEnded(stop_reason) => self.sink.work_ended(stop_reason),
                Effect::Interrupt(turn_id) => self.interrupt(turn_id),
                Effect::Own(own) => S::carry_out(self, own),
            }
        }
    }

    pub fn session_id(&self) -> String {
        self.session.locked().id().to_owned()
    }

    /// Asks the agent to stop the turn with this id. The turn ends for the chat
    /// once it answers, which it never does for a turn that is not running.
    fn interrupt(&self, turn_id: String) {
        let mut params = S::addressed(&self.session_id());
        params["turnId"] = json!(turn_id);
        let _ = self
            .rpc
            .send_for("turn/interrupt", params, Purpose::Interrupt(turn_id));
    }

    pub fn cancel(&self) {
        let effects = self.session.locked().cancel();
        self.apply(effects);
    }

    pub fn responder(&self, reply: Value) -> Respond {
        let driver = self.clone();
        Box::new(move |chosen| S::decide(&driver, &reply, chosen))
    }

    pub fn say(&self, text: &str) {
        self.sink.update(
            &self.session_id(),
            sikemux_fold::update::agent_text(text, None),
        );
    }

    /// Turn `turn` was a command answered without the model.
    pub fn answered(&self, turn: u64) {
        self.sink
            .end_turn(turn, TurnEnd::Completed(turn_answer(StopReason::EndTurn)));
    }

    /// Sends a request that begins turn `turn`, under `id` when this side names it.
    pub async fn send_turn(
        &self,
        turn: u64,
        prompt: &Prompt,
        id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        self.session.locked().turns().begin(
            turn,
            id,
            prompt.message_id.clone(),
            crate::server::chat::feed::title(&prompt.text),
        );
        match self.rpc.request(method, params).await {
            Ok(answer) => {
                if let Some(id) = S::turn_id_in(&answer).or(id) {
                    let effects = self.session.locked().started(turn, id);
                    self.apply(effects);
                }
                Ok(())
            }
            Err(error) => {
                self.session.locked().turns().abandon(turn);
                Err(error)
            }
        }
    }

    /// Writes a steer to the agent now, so a stop sent after it reaches the
    /// agent after it too. None when no turn is running.
    pub fn send_steer(
        &self,
        prompt: &Prompt,
        input: impl FnOnce() -> Result<Vec<Value>, String>,
    ) -> Result<Option<Sent>, String> {
        let Some(turn_id) = self.session.locked().turns().active() else {
            return Ok(None);
        };
        let mut params = S::addressed(&self.session_id());
        params["expectedTurnId"] = json!(turn_id);
        params["input"] = json!(input()?);
        self.rpc
            .start_for(
                "turn/steer",
                params,
                Some(Purpose::Steer {
                    text: prompt.text.clone(),
                    paths: prompt.paths.clone(),
                }),
            )
            .map(Some)
            .map_err(|error| error_text(&error))
    }

    /// What became of a steer: `injected` once it is in, `promptRequired` when
    /// the turn ended first.
    pub fn steered(
        sent: Result<Option<Sent>, String>,
    ) -> impl Future<Output = Result<String, String>> + Send + 'static {
        async move {
            let Some(sent) = sent? else {
                return Ok("promptRequired".to_owned());
            };
            match sent
                .answer(Some(ANSWER_WAIT))
                .await
                .map_err(|error| error_text(&error))
            {
                Ok(_) => Ok("injected".to_owned()),
                Err(error) if S::TURN_ENDED.iter().any(|said| error.contains(said)) => {
                    Ok("promptRequired".to_owned())
                }
                Err(error) => Err(error),
            }
        }
    }

    /// Lets go of the agent without stopping it. On failure the agent is stopped.
    pub async fn release<Kept: Serialize + Send>(
        process: Process,
        reading: Reading<Self>,
        kept: Kept,
    ) -> Result<(Value, Released), String> {
        process::hand_over(process, reading, S::NAME, |driver: Self| Handed {
            rpc: driver.rpc.state(),
            session: std::mem::take(&mut *driver.session.locked()),
            kept,
        })
        .await
    }

    /// Takes up an agent an earlier core released, from what `release` answered.
    pub fn adopt<Kept: DeserializeOwned>(
        sink: Sink,
        state: Value,
        writer: Writer,
        lines: Lines,
    ) -> Result<(Self, Reading<Self>, Kept), String> {
        let state: Handed<S, Kept> =
            serde_json::from_value(state).map_err(|error| error.to_string())?;
        let driver = Self::new(Rpc::restored(writer, state.rpc), sink, state.session);
        let (reading, _) = Reading::start(driver.clone(), lines);
        Ok((driver, reading, state.kept))
    }
}

impl<S: RpcAgent> Reads for Driver<S> {
    type Nudge = ();
    type Message = Value;

    fn message(&mut self, message: Value) {
        match self.rpc.read(message) {
            Some(Incoming::Notification { method, params }) => {
                S::notification(self, &method, params)
            }
            Some(Incoming::Request { id, method, params }) => {
                S::request(self, id, &method, &params)
            }
            Some(Incoming::Answer { purpose, result }) => match (purpose, result) {
                (Purpose::Interrupt(turn_id), Ok(_)) => {
                    let effects = self.session.locked().interrupted(&turn_id);
                    self.apply(effects);
                }
                (Purpose::Steer { text, paths }, Ok(_)) => self.sink.prompted(&text, &paths),
                (purpose, result) => S::purpose_answered(self, purpose, result),
            },
            None => {}
        }
    }

    fn closed(&mut self) {
        self.rpc.close();
    }
}
