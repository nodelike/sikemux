//! Moving a chat to another agent, account, model or permission mode, handing over what it
//! said once it has begun, and forking it.

use super::*;
use sikemux_core::server::call;
use sikemux_core::server::chat::cap::KeepHistoryParams;
use sikemux_core::server::chat::handoff::{DropHandoffParams, HandOffParams};
use sikemux_core::server::workspace::AgentPatch;
use sikemux_view::layout::{At, OpenTab, PaneKind};

impl Workbench {
    /// Moves a chat that has not started to another agent, account or provider. The model and
    /// effort it offered belong to the agent it leaves, so they go until the core says the new ones.
    pub fn move_chat(&mut self, agent: &str, to: crate::chat::model_picker::Moving) {
        if let Some(chat) = self.chats.open.get_mut(agent) {
            chat.forget_choices();
        }
        let account = to
            .profile
            .as_deref()
            .and_then(|id| self.accounts.profile(id));
        let title = account.map_or(to.provider.as_str(), |account| &account.name);
        let patch = AgentPatch {
            provider: Some(to.provider.clone()),
            profile_id: Some(to.profile.clone()),
            model: Some(to.model),
            model_provider: Some(to.model_provider),
            effort: Some(None),
            ..AgentPatch::default()
        };
        self.start_as(agent, patch, title);
    }

    /// Hands `agent`'s chat, which has begun, to the agent `to` names, or forks it into a new
    /// chat with that agent. The chat that carries on waits for the person with the handoff shown.
    pub fn hand_off(
        &mut self,
        agent: &str,
        to: crate::chat::model_picker::Moving,
        fork: bool,
        cx: &mut Context<Self>,
    ) {
        if !fork {
            if let Some(chat) = self.chats.open.get_mut(agent) {
                chat.forget_choices();
            }
        }
        let asked = (self.chat_target(agent)).request_typed::<call::HandOff>(HandOffParams {
            agent_id: agent.to_owned(),
            provider: to.provider,
            profile_id: to.profile,
            model: to.model,
            model_provider: to.model_provider,
            fork,
        });
        let agent = agent.to_owned();
        cx.spawn(async move |bench, cx| {
            let handed = asked.await;
            bench
                .update(cx, |bench, cx| match handed {
                    Ok(handed) if handed.agent_id == agent => {
                        bench.ask_chat(ChatAsk::Attach { agent, since: None });
                    }
                    Ok(handed) => bench.open_in_front(OpenTab::new(
                        At::project(&handed.project),
                        PaneKind::Agent,
                        &handed.agent_id,
                    )),
                    Err(error) => {
                        bench.notify(crate::toast::error(error), cx);
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Forks `agent`'s chat into a new chat with the same agent and account.
    pub fn fork_chat(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(saved) = self.saved_agent(agent) else {
            return;
        };
        let to = crate::chat::model_picker::Moving {
            provider: saved.provider.clone(),
            profile: saved.profile_id.clone(),
            model: saved.model.clone(),
            model_provider: saved.model_provider.clone(),
        };
        self.hand_off(agent, to, true, cx);
    }

    /// Sends `agent`'s next message without the handoff waiting for it.
    pub fn drop_handoff(&self, agent: &str) {
        (self.chat_target(agent)).call_typed::<call::DropHandoff>(DropHandoffParams {
            agent_id: agent.to_owned(),
        });
    }

    /// Summarises `agent`'s chat before its next message, or keeps its full history, once its
    /// prompt cache has expired.
    pub fn summarise_first(&self, agent: &str, summarise: bool) {
        (self.chat_target(agent)).call_typed::<call::KeepHistory>(KeepHistoryParams {
            agent_id: agent.to_owned(),
            keep: !summarise,
        });
    }

    /// The model menu of the chat in front, as its composer offers it: the model chosen and
    /// the ones it lists.
    pub fn front_models(&self) -> Option<(String, Vec<String>)> {
        let chat = self.chats.open.get(&self.agent()?.id)?;
        let model = chat.state.as_ref()?.model.as_ref()?;
        let offered = model.choices.iter().map(|choice| choice.value.clone());
        Some((model.current.clone(), offered.collect()))
    }

    /// Chooses `value` in the model menu of the chat in front, as a click on its row does.
    pub fn choose_front_model(&self, value: &str) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let Some(config) = self
            .chats
            .open
            .get(&agent)
            .and_then(|chat| Some(chat.state.as_ref()?.model.as_ref()?.id.clone()))
        else {
            return;
        };
        self.ask_chat(ChatAsk::Config {
            agent,
            config,
            value: value.to_owned(),
        });
    }

    /// Moves the chat in front to `provider` on its chosen account, as the model menu's chip for
    /// the agent does.
    pub fn move_front_to(&mut self, provider: &str) {
        let Some(agent) = self.agent().map(|agent| agent.id) else {
            return;
        };
        let to = crate::chat::model_picker::Moving {
            provider: provider.to_owned(),
            profile: self
                .accounts
                .current(provider)
                .map(|profile| profile.id.clone()),
            model: None,
            model_provider: None,
        };
        self.move_chat(&agent, to);
    }

    /// Starts `agent`, which has not started yet, again as `patch` changes it, titled `title`
    /// unless the person named it.
    pub fn start_as(&self, agent: &str, mut patch: AgentPatch, title: &str) {
        let renamed = self.shown().is_some_and(|view| {
            view.workspace
                .agents
                .iter()
                .any(|saved| saved.id == agent && saved.renamed)
        });
        patch.resume_id = Some(None);
        if !renamed {
            patch.title = Some(title.to_owned());
        }
        self.ask_chat(ChatAsk::Move {
            agent: agent.to_owned(),
            patch,
        });
    }

    /// Lets the agent of the tab in front, or of its desk, act without asking, or has it ask
    /// again, wherever the keys are.
    pub fn toggle_yolo_in_front(&self) {
        let Some(tab) = self.tab() else {
            return;
        };
        let agent = crate::tabs::shows(tab, sikemux_view::layout::PaneKind::Agent)
            .or_else(|| crate::tabs::shows(tab, sikemux_view::layout::PaneKind::Desk));
        if let Some(agent) = agent {
            self.toggle_yolo(agent);
        }
    }

    pub fn saved_agent(&self, agent: &str) -> Option<&WorkspaceAgent> {
        (self.shown()?.workspace.agents.iter()).find(|saved| saved.id == agent)
    }

    /// Starts a new chat in `agent`'s project with the same agent, account, model and mode.
    pub fn fresh_chat(&self, agent: &str) {
        let Some(saved) = self.saved_agent(agent) else {
            return;
        };
        let own =
            (saved.profile_id.as_ref()).map(|profile| format!("{}:{profile}", saved.provider));
        let launcher = own
            .filter(|own| self.launchers().iter().any(|launcher| &launcher.id == own))
            .unwrap_or_else(|| saved.provider.clone());
        self.target().start_chat_as(
            launcher,
            saved.project.clone(),
            Some(saved.permission_mode),
            (saved.model.clone(), saved.effort.clone()),
            None,
        );
    }

    /// Lets `agent` run without asking, or has it ask again, when its kind of agent can.
    pub fn toggle_yolo(&self, agent: &str) {
        let Some(saved) = self.saved_agent(agent) else {
            return;
        };
        let Some(kind) = self.reads.catalog.kind(&saved.provider) else {
            return;
        };
        if !kind.can_skip_asking(self.in_terminal(agent)) {
            return;
        }
        let auto_by_default = kind.auto
            && self
                .this_view()
                .is_some_and(|view| view.workspace.permission_mode == PermissionMode::Auto);
        let mode = match saved.permission_mode {
            PermissionMode::Bypass if auto_by_default => PermissionMode::Auto,
            PermissionMode::Bypass => PermissionMode::WorkspaceWrite,
            _ => PermissionMode::Bypass,
        };
        self.switch_mode(agent, mode);
    }
}
