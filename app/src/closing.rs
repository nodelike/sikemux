//! Closing agents stops them, so every way of closing one asks here first: when one is at
//! work, and when the agents it started could stop with it or keep running on their own. Their
//! triggers pause, and a toast says so with Undo, which takes their chats up again.

use std::rc::Rc;

use gpui_kit::Context;
use sikemux_view::layout::PaneKind;
use sikemux_view::workspace::WorkspaceAgent;

use crate::agents::rows::AgentRow;
use crate::asking::Choice;
use crate::core::Target;
use crate::toast;
use crate::workbench::{Strip, Workbench};
use sikemux_core::server::call;
use sikemux_core::server::workspace::CloseAgentsParams;

impl Workbench {
    /// Closes an agent that runs on another host, asking first while it is at work. The host
    /// it runs on closes it: asked straight when this window holds its feed, else through this
    /// host's core, which passes the call on for a worker of its own agents.
    pub fn close_elsewhere(&mut self, agent: &AgentRow, cx: &mut Context<Self>) {
        let target = match self.view_at(&agent.place) {
            Some(_) => Target::new(&self.asks, agent.place.clone()),
            None => Target::here(&self.asks),
        };
        let agent_ids = vec![agent.id.clone()];
        let close = move || {
            target.call_typed::<call::CloseAgents>(CloseAgentsParams {
                agent_ids: agent_ids.clone(),
            })
        };
        if !agent.busy {
            return close();
        }
        self.confirm(
            t!("Close {name}?", name = agent.title),
            t!("{names} stops with it.", names = agent.title.clone()),
            t!("Close"),
            true,
            move |_, yes, _, _| {
                if yes {
                    close();
                }
            },
            cx,
        );
    }

    /// Closes these agents, asking first.
    pub fn close_agents(&mut self, agents: Vec<String>, cx: &mut Context<Self>) {
        let title = match agents.as_slice() {
            [one] => t!("Close {name}?", name = self.label_of(one, false)),
            _ => t!("Close {count} agents?", count = agents.len()),
        };
        self.ask_to_stop(agents.clone(), title, cx, move |bench, workers| {
            let agent_ids = agents.iter().chain(&workers).cloned().collect();
            bench
                .target()
                .call_typed::<call::CloseAgents>(CloseAgentsParams { agent_ids });
        });
    }

    /// Closes tabs of `strip`, asking first about the agents and unsaved files in them.
    pub fn close_tabs_asking(&mut self, strip: Strip, tabs: Vec<String>, cx: &mut Context<Self>) {
        let place = self.place();
        let (mut agents, mut files) = (Vec::new(), Vec::new());
        for tab in self
            .tabs(&strip)
            .iter()
            .filter(|tab| tabs.contains(&tab.id))
        {
            agents.extend(
                crate::panes::views(&tab.pane)
                    .filter(|view| view.kind == PaneKind::Agent)
                    .map(|view| view.target.clone()),
            );
            files.extend(crate::editor::files_of(&place, &tab.pane));
        }
        let title = match (tabs.len(), agents.as_slice()) {
            (1, [one]) => t!("Close {name}?", name = self.label_of(one, false)),
            (1, _) => t!("Close this tab?").to_owned(),
            (count, _) => t!("Close {count} tabs?", count = count),
        };
        let close = move |bench: &mut Workbench, cx: &mut Context<Workbench>| {
            let (strip, tabs) = (strip.clone(), tabs.clone());
            bench.ask_to_stop(agents.clone(), title.clone(), cx, move |bench, workers| {
                bench.close_tabs(&strip, &tabs);
                if !workers.is_empty() {
                    bench
                        .target()
                        .call_typed::<call::CloseAgents>(CloseAgentsParams { agent_ids: workers });
                }
            });
        };
        crate::editor::close_unsaved(self, files, close, cx);
    }

    /// Runs `close` with the workers to stop too. When these agents started others, one
    /// question asks whether those stop or keep running; otherwise it asks only when one of
    /// them is at work.
    pub fn ask_to_stop(
        &mut self,
        agents: Vec<String>,
        title: String,
        cx: &mut Context<Self>,
        close: impl Fn(&mut Workbench, Vec<String>) + 'static,
    ) {
        let closing = agents.clone();
        let close = move |bench: &mut Workbench, workers: Vec<String>, cx: &mut Context<Self>| {
            let paused = bench.pausing(closing.iter().chain(&workers));
            close(bench, workers);
            bench.say_paused(paused, cx);
        };
        let busy: Vec<&String> = agents
            .iter()
            .filter(|id| self.agent_by_id(id).is_some_and(|row| row.busy))
            .collect();
        let workers = self.workers_of(&agents);
        if workers.is_empty() {
            if busy.is_empty() {
                return close(self, Vec::new(), cx);
            }
            let shown: Vec<String> = busy.iter().map(|id| self.label_of(id, false)).collect();
            let body = match busy.len() {
                1 => t!("{names} stops with it.", names = listed(&shown)),
                _ => t!("{names} stop with it.", names = listed(&shown)),
            };
            return self.confirm(
                title,
                body,
                t!("Close"),
                true,
                move |bench, yes, _, cx| {
                    if yes {
                        close(bench, Vec::new(), cx);
                    }
                },
                cx,
            );
        }
        let view = self.shown();
        let leads: Vec<String> = agents
            .iter()
            .filter(|id| {
                view.into_iter()
                    .flat_map(|view| &view.workspace.agents)
                    .any(|agent| workers.contains(&agent.id) && agent.lead.as_ref() == Some(id))
            })
            .map(|id| self.label_of(id, true))
            .collect();
        let busy: Vec<String> = busy.iter().map(|id| self.label_of(id, true)).collect();
        let named: Vec<String> = workers.iter().map(|id| self.label_of(id, true)).collect();
        let one = workers.len() == 1;
        let working = match busy.len() {
            0 => None,
            1 => Some(t!("{name} is working and stops with it.", name = busy[0])),
            _ => Some(t!(
                "{names} are working and stop with it.",
                names = listed(&busy)
            )),
        };
        let title = match leads.as_slice() {
            [lead] => t!("Stop {lead}'s agents too?", lead = lead),
            _ => t!("Stop their agents too?").to_owned(),
        };
        let started = match one {
            true => t!(
                "{workers} was started by {leads}. Stop it, or keep it running on its own.",
                workers = listed(&named),
                leads = listed(&leads)
            ),
            false => t!(
                "{workers} were started by {leads}. Stop them, or keep them running on their own.",
                workers = listed(&named),
                leads = listed(&leads)
            ),
        };
        let body = match working {
            Some(working) => format!("{working} {started}"),
            None => started,
        };
        self.confirm_or(
            title,
            body,
            if one { t!("Stop it") } else { t!("Stop them") },
            Some((if one { t!("Keep it") } else { t!("Keep them") }).into()),
            true,
            move |bench, choice, _, cx| match choice {
                Choice::Yes => close(bench, workers.clone(), cx),
                Choice::Other => close(bench, Vec::new(), cx),
                Choice::No => {}
            },
            cx,
        );
    }

    /// The chats of these agents whose triggers pause as they close, each with how many.
    fn pausing<'a>(&self, agents: impl Iterator<Item = &'a String>) -> Vec<(WorkspaceAgent, u32)> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        agents
            .filter_map(|id| {
                let triggers = self.agent_by_id(id)?.triggers;
                let agent = view.workspace.agents.iter().find(|agent| &agent.id == id)?;
                (triggers > 0 && agent.resume_id.is_some()).then(|| (agent.clone(), triggers))
            })
            .collect()
    }

    /// Says which triggers paused as their chats closed, with Undo taking those chats up again.
    fn say_paused(&mut self, paused: Vec<(WorkspaceAgent, u32)>, cx: &mut Context<Self>) {
        let triggers: u32 = paused.iter().map(|(_, triggers)| triggers).sum();
        let closed = match paused.as_slice() {
            [] => return,
            [(agent, _)] => self.agent_by_id(&agent.id).map_or_else(
                || t!("the chat").to_owned(),
                |row| {
                    row.handle()
                        .map_or(row.title.clone(), |name| format!("@{name}"))
                },
            ),
            more => t!("{count} chats", count = more.len()),
        };
        let text = match triggers {
            1 => t!("Closed {closed} · paused 1 trigger", closed = closed),
            _ => t!(
                "Closed {closed} · paused {count} triggers",
                closed = closed,
                count = triggers
            ),
        };
        let (bench, paused) = (cx.weak_entity(), Rc::new(paused));
        let note = toast::undoable(text, move |_, cx| {
            let paused = paused.clone();
            let _ = bench.update(cx, |bench, cx| {
                for (agent, _) in paused.iter() {
                    let Some(session) = agent.resume_id.clone() else {
                        continue;
                    };
                    let (provider, cwd) = (agent.provider.clone(), agent.cwd.clone());
                    bench
                        .resume_session(
                            &agent.project,
                            provider,
                            session,
                            cwd,
                            agent.title.clone(),
                            cx,
                        )
                        .detach();
                }
            });
        });
        self.notify(note, cx);
    }

    /// The open agents these agents started, and the ones those started, without these.
    fn workers_of(&self, agents: &[String]) -> Vec<String> {
        let Some(view) = self.shown() else {
            return Vec::new();
        };
        let mut team: Vec<&String> = agents.iter().collect();
        let mut workers = Vec::new();
        let mut found = true;
        while found {
            found = false;
            for agent in &view.workspace.agents {
                let started = agent.lead.as_ref().is_some_and(|lead| team.contains(&lead));
                if started && !team.contains(&&agent.id) {
                    team.push(&agent.id);
                    workers.push(agent.id.clone());
                    found = true;
                }
            }
        }
        workers
    }

    /// What to call an agent in a question: its name when `named` and it has one, else its title.
    fn label_of(&self, agent: &str, named: bool) -> String {
        self.agent_by_id(agent)
            .map(|row| match row.handle() {
                Some(name) if named => name.to_owned(),
                _ => row.title,
            })
            .unwrap_or_else(|| t!("the agent").to_owned())
    }
}

/// Up to three names joined in a sentence, the rest counted.
fn listed(names: &[String]) -> String {
    let mut shown: Vec<String> = names.iter().take(3).cloned().collect();
    if names.len() > 3 {
        shown.push(t!("{count} more", count = names.len() - 3));
    }
    match shown.split_last() {
        Some((last, rest)) if !rest.is_empty() => {
            t!("{names} and {last}", names = rest.join(", "), last = last)
        }
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}
