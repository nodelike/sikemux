//! A message sent while its chat's agent is still installing waits in the draft, the chat says
//! so, and it goes by itself once the install ends; a failed install says so instead, with its
//! output a click away.

use std::rc::Rc;

use gpui_kit::{Context, WeakEntity};

use super::{ComposeStage, Composing};
use crate::agents::setup::{self, Installing};
use crate::chat::composer::Notice;
use crate::hosts::Place;
use crate::workbench::Workbench;

impl Workbench {
    /// Holds `agent`'s message while its agent installs on this host; true when it waits.
    pub(crate) fn hold_for_install(&mut self, agent: &str, cx: &mut Context<Self>) -> bool {
        let Some(kind) = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
            .map(|saved| saved.provider.clone())
        else {
            return false;
        };
        let here = matches!(self.chat_target(agent).place(), Place::This);
        let state = self.agent_setup.installing(&kind).filter(|_| here);
        let Some(state) = state else {
            return false;
        };
        if let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.refusal = None;
            composing.stage = ComposeStage::Installing { agent: kind, state };
        }
        cx.notify();
        true
    }
}

/// Sends each message that waited for an install that has now ended, or says it failed.
pub(crate) fn installs_moved(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let mut ready = Vec::new();
    for (agent, chat) in bench.chats.open.iter_mut() {
        let Some(composing) = chat.composer.as_mut() else {
            continue;
        };
        let ComposeStage::Installing {
            agent: kind,
            state: Installing::Running,
        } = &composing.stage
        else {
            continue;
        };
        match bench.agent_setup.installing(kind) {
            Some(Installing::Running) => {}
            Some(Installing::Failed) => {
                composing.stage = ComposeStage::Installing {
                    agent: kind.clone(),
                    state: Installing::Failed,
                }
            }
            None => {
                composing.stage = ComposeStage::Writing;
                ready.push(agent.clone());
            }
        }
    }
    cx.notify();
    if ready.is_empty() {
        return;
    }
    let window = bench.window;
    cx.spawn(async move |bench, cx| {
        let _ = window.update(cx, |_, window, cx| {
            bench.update(cx, |bench, cx| {
                for agent in &ready {
                    bench.send(agent, false, window, cx);
                }
            })
        });
    })
    .detach();
}

/// What the composer says while its message waits for an install, or once that install failed.
pub(super) fn notice(
    bench: &Workbench,
    composing: &Composing,
    this: &WeakEntity<Workbench>,
) -> Option<Notice> {
    let ComposeStage::Installing { agent: kind, state } = &composing.stage else {
        return None;
    };
    let label = bench.agent_name(kind);
    Some(match state {
        Installing::Running => {
            Notice::Waiting(format!("Waiting for {label} to finish installing…").into())
        }
        Installing::Failed => {
            let (this, kind) = (this.clone(), kind.clone());
            Notice::Refused(
                format!("{label} did not install, so the message waits in the draft.").into(),
                Some((
                    "Show the installer's output".into(),
                    Rc::new(move |window, cx| {
                        this.update(cx, |bench, cx| {
                            setup::show_install(bench, &kind, window, cx)
                        })
                        .ok();
                    }),
                )),
            )
        }
    })
}
