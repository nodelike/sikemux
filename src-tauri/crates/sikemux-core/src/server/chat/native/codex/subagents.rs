//! Codex's subagents as sessions of their own inside the chat. Each runs on a
//! thread of its own over the same connection; the chat is told when one
//! appears and when it ends, and its updates go to its own session.

use std::collections::{HashMap, VecDeque};

use serde_json::{json, Value};

/// How many of a not yet announced subagent's notifications are held.
const HELD: usize = 256;

fn normalized(path: &str) -> &str {
    let path = path.trim().trim_end_matches('/');
    if path.is_empty() {
        "/root"
    } else {
        path
    }
}

fn is_root_path(path: &str) -> bool {
    matches!(normalized(path), "/root" | "root")
}

pub(super) fn fallback_name(thread_id: &str) -> String {
    let start = thread_id
        .char_indices()
        .rev()
        .nth(7)
        .map_or(0, |(index, _)| index);
    format!("Agent {}", &thread_id[start..])
}

/// A subagent's name from its path, such as `/root/code_reviewer` read as
/// "Code reviewer".
pub(super) fn name_from_path(path: &str, fallback: String) -> String {
    let path = normalized(path);
    let last = path.rsplit('/').next().unwrap_or(path).trim();
    let words = last
        .split(['_', '-'])
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => fallback,
    }
}

pub(super) fn spawned(session: &str, name: &str, task: &str) -> Value {
    json!({
        "sessionUpdate": "subagent_spawned",
        "subagentSessionId": session,
        "name": name,
        "task": task,
        "capabilities": {},
    })
}

pub(super) fn state_update(session: &str, state: &str) -> Value {
    json!({ "sessionUpdate": "subagent_state_update", "subagentSessionId": session, "state": state })
}

fn ended_state(status: &str) -> Option<&'static str> {
    match status {
        "completed" => Some("completed"),
        "interrupted" => Some("cancelled"),
        "errored" | "shutdown" | "notFound" | "failed" => Some("failed"),
        _ => None,
    }
}

struct Child {
    parent_thread: String,
    parent_session: String,
    session: String,
    name: String,
    task: String,
    path: String,
    generation: u32,
    ended: bool,
}

struct Pending {
    parent_thread: String,
    parent_session: String,
    task: String,
    held: VecDeque<(String, Value)>,
}

/// Where a notification goes.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// To the chat session with this id.
    Session(String),
    /// Nowhere further: it was about a subagent, and said all it had to.
    Taken,
}

/// An update for a session, and the session it is for.
pub(super) type Addressed = (String, Value);

pub(super) struct Subagents {
    root: String,
    children: HashMap<String, Child>,
    order: Vec<String>,
    pending: HashMap<String, Pending>,
    ended_pending: HashMap<String, Pending>,
}

impl Subagents {
    pub fn new(root: String) -> Self {
        Self {
            root,
            children: HashMap::new(),
            order: Vec::new(),
            pending: HashMap::new(),
            ended_pending: HashMap::new(),
        }
    }

    fn known(&self, thread: &str) -> bool {
        thread != self.root
            && (self.children.contains_key(thread)
                || self.pending.contains_key(thread)
                || self.ended_pending.contains_key(thread))
    }

    /// The session a thread's requests are asked in.
    pub fn session_of(&self, thread: &str) -> String {
        self.children
            .get(thread)
            .map_or_else(|| self.root.clone(), |child| child.session.clone())
    }

    /// Whether a subagent is still at work, or about to be.
    pub fn outstanding(&self) -> bool {
        !self.pending.is_empty() || self.children.values().any(|child| !child.ended)
    }

    /// Routes one notification. Updates about subagents go to `out`, and
    /// notifications held for a subagent that has now appeared go to `replay`
    /// to be read again.
    pub fn route(
        &mut self,
        method: &str,
        params: &Value,
        out: &mut Vec<Addressed>,
        replay: &mut Vec<(String, Value)>,
    ) -> Route {
        let thread = params["threadId"].as_str().unwrap_or(&self.root).to_owned();
        match method {
            "turn/started" if self.known(&thread) => return Route::Taken,
            "turn/completed" if self.known(&thread) => {
                if let Some(state) = params["turn"]["status"].as_str().and_then(ended_state) {
                    if self.pending.contains_key(&thread) {
                        self.end_pending(&thread);
                    } else {
                        self.finish(&thread, state, out);
                    }
                }
                return Route::Taken;
            }
            _ => {}
        }
        if let Some(pending) = self.pending.get_mut(&thread) {
            if pending.held.len() == HELD {
                pending.held.pop_front();
            }
            pending.held.push_back((method.to_owned(), params.clone()));
            return Route::Taken;
        }
        if matches!(method, "item/started" | "item/completed")
            && self.claims(&params["item"], out, replay)
        {
            return Route::Taken;
        }
        if self.ended_pending.contains_key(&thread)
            || self.children.get(&thread).is_some_and(|child| child.ended)
        {
            return Route::Taken;
        }
        Route::Session(self.session_of(&thread))
    }

    /// Whether an item was about subagents and is said by them alone.
    fn claims(
        &mut self,
        item: &Value,
        out: &mut Vec<Addressed>,
        replay: &mut Vec<(String, Value)>,
    ) -> bool {
        match item["type"].as_str() {
            Some("subAgentActivity") => {
                let path = item["agentPath"].as_str().unwrap_or_default();
                let Some(child) = item["agentThreadId"].as_str() else {
                    return true;
                };
                if is_root_path(path) || self.ended_pending.contains_key(child) {
                    return true;
                }
                if !self.children.contains_key(child) {
                    self.materialize(child, path, out, replay);
                }
                if item["kind"] == "interrupted" {
                    self.finish(child, "cancelled", out);
                }
                true
            }
            Some("collabAgentToolCall") => self.collaboration(item, out),
            _ => false,
        }
    }

    fn collaboration(&mut self, item: &Value, out: &mut Vec<Addressed>) -> bool {
        let tool = item["tool"].as_str().unwrap_or_default();
        let states = item["agentsStates"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if matches!(tool, "resumeAgent" | "sendInput") {
            for (child, state) in &states {
                if matches!(state["status"].as_str(), Some("running" | "pendingInit")) {
                    self.reopen(child, out);
                }
            }
        }
        let receivers: Vec<String> = item["receiverThreadIds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let mut represented = false;
        if tool == "spawnAgent" {
            let sender = item["senderThreadId"].as_str().unwrap_or_default();
            let (parent_thread, parent_session) = match self.children.get(sender) {
                Some(parent) => (sender.to_owned(), parent.session.clone()),
                None => (self.root.clone(), self.root.clone()),
            };
            let task = item["prompt"]
                .as_str()
                .map(str::trim)
                .filter(|prompt| !prompt.is_empty())
                .unwrap_or("Delegated task")
                .to_owned();
            for child in &receivers {
                if child.trim().is_empty() || *child == parent_session || *child == self.root {
                    continue;
                }
                represented = true;
                if self.known(child) {
                    continue;
                }
                self.pending.insert(
                    child.clone(),
                    Pending {
                        parent_thread: parent_thread.clone(),
                        parent_session: parent_session.clone(),
                        task: task.clone(),
                        held: VecDeque::new(),
                    },
                );
            }
        }
        for (child, state) in &states {
            let Some(state) = state["status"].as_str().and_then(ended_state) else {
                continue;
            };
            if self.children.contains_key(child) {
                self.finish(child, state, out);
            } else {
                self.end_pending(child);
            }
        }
        if tool == "spawnAgent" && item["status"] == "failed" {
            for child in &receivers {
                self.end_pending(child);
            }
        }
        tool == "spawnAgent" && represented
    }

    fn parent_for_path(&self, path: &str) -> (String, String) {
        let path = normalized(path);
        let Some(parent_path) = path.rfind('/').filter(|at| *at > 0).map(|at| &path[..at]) else {
            return (self.root.clone(), self.root.clone());
        };
        self.children
            .iter()
            .find(|(_, child)| child.path == parent_path)
            .map_or_else(
                || (self.root.clone(), self.root.clone()),
                |(thread, child)| (thread.clone(), child.session.clone()),
            )
    }

    fn materialize(
        &mut self,
        child: &str,
        path: &str,
        out: &mut Vec<Addressed>,
        replay: &mut Vec<(String, Value)>,
    ) {
        let pending = self.pending.remove(child);
        let name = name_from_path(path, fallback_name(child));
        let (parent_thread, parent_session, task, held) = match pending {
            Some(pending) => (
                pending.parent_thread,
                pending.parent_session,
                pending.task,
                pending.held,
            ),
            None => {
                let (thread, session) = self.parent_for_path(path);
                (
                    thread,
                    session,
                    format!("Delegated task for {name}"),
                    VecDeque::new(),
                )
            }
        };
        out.push((parent_session.clone(), spawned(child, &name, &task)));
        self.children.insert(
            child.to_owned(),
            Child {
                parent_thread,
                parent_session,
                session: child.to_owned(),
                name,
                task,
                path: normalized(path).to_owned(),
                generation: 1,
                ended: false,
            },
        );
        self.order.push(child.to_owned());
        replay.extend(held);
    }

    fn end_pending(&mut self, child: &str) {
        if let Some(pending) = self.pending.remove(child) {
            self.ended_pending.insert(child.to_owned(), pending);
        }
    }

    fn finish(&mut self, child: &str, state: &str, out: &mut Vec<Addressed>) {
        if let Some(child) = self.children.get_mut(child).filter(|child| !child.ended) {
            child.ended = true;
            out.push((
                child.parent_session.clone(),
                state_update(&child.session, state),
            ));
        }
    }

    fn reopen(&mut self, thread: &str, out: &mut Vec<Addressed>) {
        if !self.children.contains_key(thread) {
            let Some(pending) = self.ended_pending.remove(thread) else {
                return;
            };
            let parent_session = self
                .children
                .get(&pending.parent_thread)
                .map_or_else(|| self.root.clone(), |parent| parent.session.clone());
            let session = format!("{thread}:generation:2");
            let name = fallback_name(thread);
            out.push((
                parent_session.clone(),
                spawned(&session, &name, &pending.task),
            ));
            self.children.insert(
                thread.to_owned(),
                Child {
                    parent_thread: pending.parent_thread,
                    parent_session,
                    session,
                    name,
                    task: pending.task,
                    path: String::new(),
                    generation: 2,
                    ended: false,
                },
            );
            self.order.push(thread.to_owned());
            return;
        }
        let parent_session = self
            .children
            .get(thread)
            .and_then(|child| self.children.get(&child.parent_thread))
            .map_or_else(|| self.root.clone(), |parent| parent.session.clone());
        let Some(child) = self.children.get_mut(thread).filter(|child| child.ended) else {
            return;
        };
        child.parent_session = parent_session;
        child.generation += 1;
        child.session = format!("{thread}:generation:{}", child.generation);
        child.ended = false;
        out.push((
            child.parent_session.clone(),
            spawned(&child.session, &child.name, &child.task),
        ));
    }

    /// Ends every subagent still at work, as the turn that started them ended.
    pub fn finish_outstanding(&mut self, state: &str, out: &mut Vec<Addressed>) {
        let pending: Vec<String> = self.pending.keys().cloned().collect();
        for child in pending {
            self.end_pending(&child);
        }
        for child in self.order.clone().iter().rev() {
            self.finish(child, state, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(
        subagents: &mut Subagents,
        method: &str,
        params: Value,
    ) -> (Route, Vec<Addressed>, Vec<(String, Value)>) {
        let mut out = Vec::new();
        let mut replay = Vec::new();
        let route = subagents.route(method, &params, &mut out, &mut replay);
        (route, out, replay)
    }

    fn spawn(sender: &str, child: &str) -> Value {
        json!({ "threadId": sender, "item": {
            "type": "collabAgentToolCall", "id": "c1", "tool": "spawnAgent", "status": "completed",
            "senderThreadId": sender, "receiverThreadIds": [child], "prompt": "Review the diff",
            "agentsStates": { child: { "status": "pendingInit" } },
        } })
    }

    fn activity(parent: &str, child: &str, kind: &str) -> Value {
        json!({ "threadId": parent, "item": {
            "type": "subAgentActivity", "id": "a1", "kind": kind,
            "agentThreadId": child, "agentPath": "/root/code_reviewer",
        } })
    }

    #[test]
    fn a_spawned_subagent_appears_with_what_it_said_before() {
        let mut subagents = Subagents::new("root".into());
        let (route, out, _) = send(&mut subagents, "item/started", spawn("root", "kid"));
        assert_eq!(route, Route::Taken);
        assert!(out.is_empty());
        assert!(subagents.outstanding());

        let early = json!({ "threadId": "kid", "itemId": "m", "delta": "hi" });
        let (held, _, _) = send(&mut subagents, "item/agentMessage/delta", early.clone());
        assert_eq!(held, Route::Taken);

        let (route, out, replay) = send(
            &mut subagents,
            "item/started",
            activity("root", "kid", "started"),
        );
        assert_eq!(route, Route::Taken);
        assert_eq!(
            out,
            vec![(
                "root".to_owned(),
                spawned("kid", "Code reviewer", "Review the diff")
            )]
        );
        assert_eq!(
            replay,
            vec![("item/agentMessage/delta".to_owned(), early.clone())]
        );

        let (route, _, _) = send(&mut subagents, "item/agentMessage/delta", early);
        assert_eq!(route, Route::Session("kid".into()));

        let done = json!({ "threadId": "kid", "turn": { "id": "t", "status": "completed" } });
        let (route, out, _) = send(&mut subagents, "turn/completed", done);
        assert_eq!(route, Route::Taken);
        assert_eq!(
            out,
            vec![("root".to_owned(), state_update("kid", "completed"))]
        );
        assert!(!subagents.outstanding());

        let late = json!({ "threadId": "kid", "itemId": "m", "delta": "late" });
        assert_eq!(
            send(&mut subagents, "item/agentMessage/delta", late).0,
            Route::Taken
        );
    }

    #[test]
    fn a_subagent_given_more_work_comes_back_as_a_new_generation() {
        let mut subagents = Subagents::new("root".into());
        send(&mut subagents, "item/started", spawn("root", "kid"));
        send(
            &mut subagents,
            "item/started",
            activity("root", "kid", "started"),
        );
        send(
            &mut subagents,
            "turn/completed",
            json!({ "threadId": "kid", "turn": { "status": "completed" } }),
        );
        let resend = json!({ "threadId": "root", "item": {
            "type": "collabAgentToolCall", "id": "c2", "tool": "sendInput", "status": "inProgress",
            "senderThreadId": "root", "receiverThreadIds": ["kid"], "prompt": "Again",
            "agentsStates": { "kid": { "status": "running" } },
        } });
        let (route, out, _) = send(&mut subagents, "item/started", resend);
        assert_eq!(route, Route::Session("root".into()));
        assert_eq!(out[0].1["subagentSessionId"], "kid:generation:2");
        assert_eq!(subagents.session_of("kid"), "kid:generation:2");
    }

    #[test]
    fn a_stopped_turn_ends_its_subagents() {
        let mut subagents = Subagents::new("root".into());
        send(&mut subagents, "item/started", spawn("root", "a"));
        send(
            &mut subagents,
            "item/started",
            activity("root", "a", "started"),
        );
        send(&mut subagents, "item/started", spawn("root", "b"));
        let mut out = Vec::new();
        subagents.finish_outstanding("cancelled", &mut out);
        assert_eq!(
            out,
            vec![("root".to_owned(), state_update("a", "cancelled"))]
        );
        assert!(!subagents.outstanding());
    }

    #[test]
    fn the_root_and_strangers_go_to_the_root_session() {
        let mut subagents = Subagents::new("root".into());
        let (route, _, _) = send(
            &mut subagents,
            "turn/started",
            json!({ "threadId": "root" }),
        );
        assert_eq!(route, Route::Session("root".into()));
        let (route, _, _) = send(
            &mut subagents,
            "item/agentMessage/delta",
            json!({ "threadId": "other" }),
        );
        assert_eq!(route, Route::Session("root".into()));
        let mut itself = activity("root", "x", "started");
        itself["item"]["agentPath"] = json!("/root");
        let (route, out, _) = send(&mut subagents, "item/started", itself);
        assert_eq!(route, Route::Taken);
        assert!(out.is_empty());
    }

    #[test]
    fn names_come_from_the_path() {
        assert_eq!(
            name_from_path("/root/code_reviewer/", "x".into()),
            "Code reviewer"
        );
        assert_eq!(name_from_path("/root/a-b  c", "x".into()), "A b c");
        assert_eq!(name_from_path("/root/", "x".into()), "Root");
        assert_eq!(fallback_name("0123456789abcdef"), "Agent 89abcdef");
        assert_eq!(fallback_name("abc"), "Agent abc");
    }
}
