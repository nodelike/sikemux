//! Closing agents stops them, so every way of closing one asks here first: when one is at
//! work, and when the agents it started could stop with it or keep running on their own.

use gpui_kit::Context;
use serde_json::json;
use sikemux_wire::protocol::PaneKind;

use crate::workbench::{Choice, Strip, Workbench};

impl Workbench {
    /// Closes these agents, asking first.
    pub fn close_agents(&mut self, agents: Vec<String>, cx: &mut Context<Self>) {
        let title = match agents.as_slice() {
            [one] => format!("Close {}?", self.label_of(one, false)),
            _ => format!("Close {} agents?", agents.len()),
        };
        self.ask_to_stop(agents.clone(), title, cx, move |bench, workers| {
            let closing: Vec<&String> = agents.iter().chain(&workers).collect();
            bench.call("closeAgents", json!({ "agentIds": closing }));
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
            (1, [one]) => format!("Close {}?", self.label_of(one, false)),
            (1, _) => "Close this tab?".to_owned(),
            (count, _) => format!("Close {count} tabs?"),
        };
        let close = move |bench: &mut Workbench, cx: &mut Context<Workbench>| {
            let (strip, tabs) = (strip.clone(), tabs.clone());
            bench.ask_to_stop(agents.clone(), title.clone(), cx, move |bench, workers| {
                bench.close_tabs(&strip, &tabs);
                if !workers.is_empty() {
                    bench.call("closeAgents", json!({ "agentIds": workers }));
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
        let busy: Vec<&String> = agents
            .iter()
            .filter(|id| self.agent_by_id(id).is_some_and(|row| row.busy))
            .collect();
        let workers = self.workers_of(&agents);
        if workers.is_empty() {
            if busy.is_empty() {
                return close(self, Vec::new());
            }
            let shown: Vec<String> = busy.iter().map(|id| self.label_of(id, false)).collect();
            let one = busy.len() == 1;
            let body = format!(
                "{} {} with it.",
                listed(&shown),
                if one { "stops" } else { "stop" }
            );
            return self.confirm(
                title,
                body,
                "Close",
                true,
                move |bench, yes, _, _| {
                    if yes {
                        close(bench, Vec::new());
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
            0 => String::new(),
            1 => format!("{} is working and stops with it. ", busy[0]),
            _ => format!("{} are working and stop with it. ", listed(&busy)),
        };
        let title = match leads.as_slice() {
            [lead] => format!("Stop {lead}'s agents too?"),
            _ => "Stop their agents too?".to_owned(),
        };
        let body = format!(
            "{working}{} {} started by {}. Stop {}, or keep {} running on {} own.",
            listed(&named),
            if one { "was" } else { "were" },
            leads.join(" and "),
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
            if one { "its" } else { "their" },
        );
        self.confirm_or(
            title,
            body,
            if one { "Stop it" } else { "Stop them" },
            Some((if one { "Keep it" } else { "Keep them" }).into()),
            true,
            move |bench, choice, _, _| match choice {
                Choice::Yes => close(bench, workers.clone()),
                Choice::Other => close(bench, Vec::new()),
                Choice::No => {}
            },
            cx,
        );
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
            .map(|row| match row.name {
                Some(name) if named => name,
                _ => row.title,
            })
            .unwrap_or_else(|| "the agent".to_owned())
    }
}

/// Up to three names joined in a sentence, the rest counted.
fn listed(names: &[String]) -> String {
    let mut shown: Vec<String> = names.iter().take(3).cloned().collect();
    if names.len() > 3 {
        shown.push(format!("{} more", names.len() - 3));
    }
    match shown.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        Some((last, _)) => last.clone(),
        None => String::new(),
    }
}
