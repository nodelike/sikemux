//! Moving a chat that has not started to another agent, account, model or permission mode.

use super::*;

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
        let patch = json!({
            "provider": to.provider,
            "profileId": to.profile,
            "model": to.model,
            "modelProvider": to.model_provider,
            "effort": null,
        });
        self.start_as(agent, patch, title);
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
    pub fn start_as(&self, agent: &str, mut patch: Value, title: &str) {
        let renamed = self.shown().is_some_and(|view| {
            view.workspace
                .agents
                .iter()
                .any(|saved| saved.id == agent && saved.renamed)
        });
        patch["resumeId"] = Value::Null;
        if !renamed {
            patch["title"] = title.into();
        }
        self.ask_chat(ChatAsk::Move {
            agent: agent.to_owned(),
            patch,
        });
    }

    /// Lets `agent` run without asking, or has it ask again, when its kind of agent can.
    /// Lets the agent of the tab in front, or of its desk, act without asking, or has it ask
    /// again, wherever the keys are.
    pub fn toggle_yolo_in_front(&self) {
        let Some(tab) = self.tab() else {
            return;
        };
        let agent = crate::workbench::shows(tab, sikemux_wire::protocol::PaneKind::Agent)
            .or_else(|| crate::workbench::shows(tab, sikemux_wire::protocol::PaneKind::Desk));
        if let Some(agent) = agent {
            self.toggle_yolo(agent);
        }
    }

    pub fn toggle_yolo(&self, agent: &str) {
        let Some(saved) = self
            .shown()
            .and_then(|view| view.workspace.agents.iter().find(|saved| saved.id == agent))
        else {
            return;
        };
        let can = self
            .reads
            .catalog
            .kind(&saved.provider)
            .is_some_and(|kind| kind.bypass);
        if can {
            let on = saved.permission_mode == "bypass";
            let mode = if on {
                PermissionMode::WorkspaceWrite
            } else {
                PermissionMode::Bypass
            };
            self.switch_mode(agent, mode);
        }
    }
}
