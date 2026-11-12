//! The chats the person has had in front, in order, for ⌘[ and ⌘] to go back and forward
//! through as a browser does through pages.

use std::time::{Duration, Instant};

use gpui_kit::Context;

use crate::workbench::Workbench;

/// The most chats kept on either side of the one in front.
const KEPT: usize = 50;
/// How long a step waits for its chat to come to the front before the chat in front counts
/// as visited again.
const STEP_ARRIVES: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Visits {
    back: Vec<String>,
    now: Option<String>,
    forward: Vec<String>,
    /// The chat a step back or forward is bringing to the front, and when it was asked for.
    stepping: Option<(String, Instant)>,
}

impl Visits {
    fn arrive(&mut self, agent: String) {
        if let Some((stepped, at)) = &self.stepping {
            if *stepped != agent && at.elapsed() < STEP_ARRIVES {
                return;
            }
            self.stepping = None;
        }
        if self.now.as_ref() == Some(&agent) {
            return;
        }
        if let Some(was) = self.now.replace(agent) {
            self.back.push(was);
            if self.back.len() > KEPT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }
}

impl Workbench {
    /// Notes the chat in front as visited.
    pub(crate) fn note_chat(&mut self) {
        if let Some(agent) = self.agent() {
            self.chats.visits.arrive(agent.id);
        }
    }

    /// Brings forward the chat visited before the one in front, or after it for `by` of 1,
    /// passing over chats that have gone since.
    pub(crate) fn step_chat(&mut self, by: isize, cx: &mut Context<Self>) {
        loop {
            let visits = &mut self.chats.visits;
            let from = match by < 0 {
                true => &mut visits.back,
                false => &mut visits.forward,
            };
            let Some(agent) = from.pop() else {
                return;
            };
            if self.agent_by_id(&agent).is_none() {
                continue;
            }
            let visits = &mut self.chats.visits;
            let to = match by < 0 {
                true => &mut visits.forward,
                false => &mut visits.back,
            };
            to.extend(visits.now.replace(agent.clone()));
            visits.stepping = Some((agent.clone(), Instant::now()));
            self.open_agent(&agent, cx);
            return;
        }
    }
}
