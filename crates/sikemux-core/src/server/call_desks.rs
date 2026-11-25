//! Which chats show the person each open call.

use std::collections::{HashMap, HashSet};

use super::Core;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use sikemux_view::calls::PersonCall;

/// Names on each call the chats that show it, the one that opens it first: an agent no agent
/// started shows its own, a worker's is on the desk of the lead at the top, and its project's
/// chief shows every one. A closed lead still counts as one, so a worker shows a call only when
/// nobody above it is open.
pub(crate) fn place<'a>(core: &Core, calls: impl IntoIterator<Item = &'a mut PersonCall>) {
    let mut calls = calls.into_iter().peekable();
    if calls.peek().is_none() {
        return;
    }
    let above = Above::new(core);
    let chiefs = super::chief::seated(core);
    let chief_at: HashMap<String, &String> = (core.workspaces().projects().into_iter())
        .filter_map(|project| {
            let chief = chiefs.get(&project.id)?;
            Some((project.path.to_string_lossy().into_owned(), chief))
        })
        .collect();
    for call in calls {
        let chains: Vec<Vec<String>> = std::iter::once(&call.asker)
            .chain(&call.joined)
            .map(|asker| above.chain(&asker.agent_id))
            .collect();
        let of_about = (call.about.iter())
            .map(|about| above.chain(&about.agent_id))
            .filter(|chain| chain.len() > 1);
        let mut shown_in: Vec<String> = Vec::new();
        let tops = (chains.iter().filter_map(|chain| above.top(chain)))
            .chain(chief_at.get(&call.project).map(|chief| (*chief).clone()))
            .chain(of_about.filter_map(|chain| above.top(&chain)));
        for desk in tops {
            if !shown_in.contains(&desk) {
                shown_in.push(desk);
            }
        }
        if shown_in.is_empty() {
            let open = (chains.iter().flat_map(|chain| chain.iter().rev()))
                .find(|agent| above.open.contains(*agent));
            shown_in.push(open.unwrap_or(&call.asker.agent_id).clone());
        }
        call.shown_in = shown_in;
    }
}

/// `call` with the chats that show it named.
pub(crate) fn placed(core: &Core, mut call: PersonCall) -> PersonCall {
    place(core, [&mut call]);
    call
}

/// Who started whom, and who is open.
struct Above<'a> {
    core: &'a Core,
    leads: HashMap<String, String>,
    open: HashSet<String>,
}

impl<'a> Above<'a> {
    fn new(core: &'a Core) -> Self {
        Self {
            core,
            leads: core.workspaces().agent_leads(),
            open: core.workspaces().agent_ids(),
        }
    }

    /// `agent`, then its lead, and so on up. A chief has no lead.
    fn chain(&self, agent: &str) -> Vec<String> {
        let mut chain = vec![agent.to_owned()];
        let mut at = agent.to_owned();
        while !super::chief::is_chief(self.core, &at) {
            match self.lead_of(&at).filter(|lead| !chain.contains(lead)) {
                Some(lead) => {
                    chain.push(lead.clone());
                    at = lead;
                }
                None => break,
            }
        }
        chain
    }

    fn lead_of(&self, agent: &str) -> Option<String> {
        (self.core.names().leads_of(agent).into_iter().next())
            .or_else(|| self.leads.get(agent).cloned())
            .or_else(|| self.core.hosts().team.get(agent)?.parent)
    }

    /// The agent at the top of `chain`, when it is open.
    fn top(&self, chain: &[String]) -> Option<String> {
        chain.last().filter(|top| self.open.contains(*top)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::names::{Starting, Worker};
    use crate::server::person_calls::{self, Raise};
    use crate::server::workspace::{AddProjectParams, AddProjectParamsExt};
    use crate::server::ServerCore;
    use serde_json::json;
    use sikemux_view::calls::{CallAsker, CallKind};
    use std::path::Path;
    use std::sync::Arc;

    struct Team {
        core: Arc<Core>,
        dir: tempfile::TempDir,
        project: String,
        path: String,
    }

    impl Team {
        async fn new() -> Self {
            let core = Core::new(Default::default(), None).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let folder = AddProjectParams::at(dir.path().to_owned());
            let project = crate::server::workspace::add_project(&core, folder)
                .await
                .expect("the project");
            let path = project.path.to_string_lossy().into_owned();
            Self {
                core,
                dir,
                project: project.id,
                path,
            }
        }

        fn open(&self, id: &str, lead: Option<&str>) {
            match lead {
                Some(lead) => {
                    let worker = Worker {
                        agent_id: id,
                        lead,
                        project: None,
                        provider: "claude",
                        session: None,
                        reports: true,
                        task: None,
                    };
                    (self.core.names())
                        .hire(worker, HashSet::new())
                        .expect("named");
                }
                None => {
                    let starting = Starting {
                        agent_id: id,
                        project: self.path.clone(),
                        provider: None,
                        resume_id: None,
                    };
                    self.core.names().give(starting, HashSet::new());
                }
            }
            let cwd: &Path = self.dir.path();
            let agent = json!({
                "id": id,
                "provider": "claude",
                "project": self.project,
                "cwd": cwd,
                "title": id,
                "lead": lead,
                "permissionMode": "auto",
            });
            let agent = agent.as_object().cloned().expect("an object");
            (self.core.workspaces())
                .put_agent_draft(agent)
                .expect("the agent");
        }

        fn chief(&self) {
            self.open("chief", None);
            (self.core.workspaces())
                .set_chief(&self.project, "chief", 1)
                .expect("the chief");
        }

        fn close(&self, id: &str) {
            crate::server::workspace::close(&self.core, &[id.to_owned()]).expect("closed");
        }

        fn raise(&self, agent: &str) -> String {
            let raise = Raise {
                project: self.path.clone(),
                kind: CallKind::Money,
                asker: CallAsker {
                    agent_id: agent.to_owned(),
                    name: agent.to_owned(),
                    cli: "claude".into(),
                    chief: false,
                },
                question: "Buy it?".into(),
                context: None,
                options: Vec::new(),
                pick: None,
                against: None,
                evidence: Vec::new(),
                action: None,
                brief: None,
                place: None,
                key: None,
                yes: None,
                attempt: None,
            };
            person_calls::raise(&self.core, raise).expect("raised")
        }

        fn desks(&self, call: &str) -> Vec<String> {
            let mut calls = person_calls::shown(&self.core);
            place(&self.core, &mut calls);
            let call = (calls.into_iter())
                .find(|shown| shown.id == call)
                .expect("shown");
            assert_eq!(call.desk(), call.shown_in[0]);
            call.shown_in
        }

        fn desks_of(&self, agent: &str) -> Vec<String> {
            self.desks(&self.raise(agent))
        }
    }

    /// The person found workers' asks on the workers' own desks and asked for the chief's only.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_worker_shows_no_ask_under_a_chief() {
        let team = Team::new().await;
        team.chief();
        team.open("lead", Some("chief"));
        team.open("worker", Some("lead"));
        assert_eq!(team.desks_of("worker"), ["chief"]);
        assert_eq!(team.desks_of("lead"), ["chief"]);
        assert_eq!(team.desks_of("chief"), ["chief"]);
        team.close("lead");
        assert_eq!(team.desks_of("worker"), ["chief"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_chat_the_person_opened_shows_its_own_and_its_workers() {
        let team = Team::new().await;
        team.chief();
        team.open("chat", None);
        team.open("middle", Some("chat"));
        team.open("worker", Some("middle"));
        assert_eq!(team.desks_of("chat"), ["chat", "chief"]);
        assert_eq!(team.desks_of("middle"), ["chat", "chief"]);
        assert_eq!(team.desks_of("worker"), ["chat", "chief"]);
        team.close("chat");
        assert_eq!(team.desks_of("worker"), ["chief"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_no_chief_the_highest_lead_open_shows_it() {
        let team = Team::new().await;
        team.open("chat", None);
        team.open("middle", Some("chat"));
        team.open("worker", Some("middle"));
        let call = team.raise("worker");
        assert_eq!(team.desks(&call), ["chat"]);
        team.close("chat");
        assert_eq!(team.desks(&call), ["middle"]);
        team.close("middle");
        assert_eq!(team.desks(&call), ["worker"]);
        team.close("worker");
        assert_eq!(team.desks(&call), ["worker"]);
        team.chief();
        assert_eq!(team.desks(&call), ["chief"]);
    }
}
