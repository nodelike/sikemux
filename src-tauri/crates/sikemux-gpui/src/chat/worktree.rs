//! Whether a new chat starts in a worktree of its own: the core says what the person chose
//! for its project, the switch in the strip on the chat's composer flips that, and the first message with
//! it on has the core make the worktree and start the chat there before it goes.

use std::rc::Rc;

use gpui_kit::{Context, Window};
use serde_json::{json, Map, Value};

use super::Chat;
use crate::core::ChatAsk;
use crate::workbench::Workbench;
use crate::worktree_switch::{Flip, Switch};

impl Workbench {
    /// The worktree switch `agent`'s composer strip shows, and what flipping it does.
    pub fn worktree_switch(&self, agent: &str, cx: &Context<Self>) -> Option<(Switch, Flip)> {
        let saved = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))?;
        let project = self
            .projects()
            .iter()
            .find(|project| project.id == saved.project)?;
        let chat = self.chats.get(agent);
        let started = saved.resume_id.is_some() || chat.is_none_or(|chat| !chat.is_empty(cx));
        let preparing = chat
            .and_then(|chat| chat.composer.as_ref())
            .and_then(|composing| composing.preparing.as_deref());
        let on = project.worktree_default;
        let switch = crate::worktree_switch::state(
            saved.worktree.as_ref(),
            project.git.is_some(),
            started,
            preparing,
            on,
        )?;
        let mut defaults = self.preferences().agent_worktree_defaults;
        defaults.insert(project.path.to_string_lossy().into_owned(), !on);
        let flipped: Map<String, Value> = defaults
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(folder, on)| (folder, Value::Bool(on)))
            .collect();
        let asks = self.asks.clone();
        let flip: Flip = Rc::new(move |_, _| {
            let _ = asks.send(crate::core::Ask::Call {
                method: "setPreference",
                params: json!({ "key": "agentWorktreeDefaults", "value": flipped }),
            });
        });
        Some((switch, flip))
    }

    /// Whether `agent`'s next message first has the core make it a worktree.
    pub(crate) fn starts_in_worktree(&self, agent: &str, cx: &Context<Self>) -> bool {
        self.worktree_switch(agent, cx)
            .is_some_and(|(switch, _)| matches!(switch, Switch::Choosing { on: true }))
    }

    /// Has the core make `agent` a worktree named after `message` and start its chat there;
    /// the message goes once the chat is ready in it.
    pub(crate) fn begin_worktree(&mut self, agent: &str, message: String, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        composing.preparing = Some("Creating worktree".into());
        composing.refusal = None;
        let made = self.chat_target(agent).request(
            "createAgentWorktree",
            json!({ "agentId": agent, "message": message }),
        );
        let agent = agent.to_owned();
        cx.spawn(async move |bench, cx| {
            let made = made.await;
            bench
                .update(cx, |bench, cx| bench.worktree_made(&agent, made, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn worktree_made(&mut self, agent: &str, made: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        else {
            return;
        };
        let refused = match &made {
            Err(error) => Some(format!("Could not create a worktree: {error}")),
            Ok(made) => made["setupError"].as_str().map(|error| {
                let path = made["worktree"]["path"].as_str().unwrap_or_default();
                format!("{error}. The worktree is kept at {path}; send again to start there.")
            }),
        };
        match refused {
            Some(refused) => {
                composing.preparing = None;
                composing.refusal = Some(refused.into());
            }
            None => {
                composing.preparing = Some("Starting the agent in the worktree".into());
                composing.send_when_ready = true;
                self.ask_chat(ChatAsk::Attach {
                    agent: agent.to_owned(),
                    since: None,
                });
            }
        }
        cx.notify();
    }

    /// Sends the message that waited for `agent`'s worktree, once the core shows the agent in
    /// it and its chat is ready there.
    pub(crate) fn send_after_worktree(
        &mut self,
        agent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let moved = self.shown().is_some_and(|view| {
            view.workspace
                .agents
                .iter()
                .any(|saved| saved.id == agent && saved.worktree.is_some())
        });
        let ready = moved && self.chats.get(agent).is_some_and(Chat::ready_in_worktree);
        let Some(composing) = self
            .chats
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
            .filter(|composing| composing.send_when_ready && ready)
        else {
            return;
        };
        composing.send_when_ready = false;
        composing.preparing = None;
        self.send(agent, false, window, cx);
    }
}

impl Chat {
    fn ready_in_worktree(&self) -> bool {
        self.state.as_ref().is_some_and(|head| {
            head.connection == sikemux_wire::chat::ChatConnection::Ready && !head.running
        })
    }
}
