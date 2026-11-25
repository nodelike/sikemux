//! How agents an agent or Auto starts are launched: on whose account, in which mode, and which
//! launcher reviews.

use super::*;
use crate::agent_accounts::by_hand;

/// Why nothing can start when the core found no agent at all.
const NO_AGENTS: &str = "Sikemux found no agent it can start on this host; install one first";

/// How a lead runs, for an agent it starts to run the same way.
pub struct Lead {
    pub provider: String,
    /// Its launch, when it is a chat the core runs.
    pub launch: Option<ChatLaunch>,
    pub launcher: Option<String>,
    pub permission_mode: Option<PermissionMode>,
}

impl Lead {
    /// The CLI it runs under, for a lead the core knows of.
    pub fn cli(&self) -> Option<&str> {
        Some(self.provider.as_str()).filter(|provider| !provider.is_empty())
    }

    /// Its CLI as a worker is told of it.
    pub fn cli_word(&self) -> &str {
        self.cli().unwrap_or("agent")
    }

    /// The permission mode a worker of the lead takes: the lead's own, or
    /// Normal when the lead's is unknown.
    pub fn worker_mode(&self) -> PermissionMode {
        self.permission_mode.unwrap_or_default()
    }
}

/// Who pays for a worker's turns.
pub enum Paid {
    /// Whoever pays for its lead's, when it runs the lead's agent.
    AsLead,
    /// The provider named, or the agent's own account.
    By(Option<String>),
}

/// The program the core last found for `launcher`, while it is still there.
pub fn still_there(launcher: &Launcher) -> Result<PathBuf, String> {
    launcher
        .program
        .clone()
        .filter(|program| program.is_file())
        .ok_or_else(|| format!("{} is no longer installed on this host", launcher.label))
}

impl Workspaces {
    /// How to start an agent of `provider` for `lead` in `cwd`: on the lead's
    /// account and in its permission mode, paid as `paid` says. The lead may
    /// pick the model.
    pub fn worker_launch(
        &self,
        lead: &Lead,
        provider: &str,
        cwd: &Path,
        model: Option<String>,
        paid: Paid,
    ) -> Result<(ChatLaunch, Option<String>), String> {
        let same = lead.provider == provider;
        let lead_own = lead.launch.as_ref().filter(|_| same);
        let lead_route = lead_own.and_then(|own| own.model_provider.clone());
        let route = match paid {
            Paid::AsLead => lead_route.clone(),
            Paid::By(route) => route,
        };
        let as_lead = route == lead_route;
        let private_home = crate::agent_catalog::agent(provider)
            .and_then(|agent| agent.home.as_ref())
            .is_some_and(|home| home.private.is_some());
        let by_hand = self.chosen_by_hand(provider);
        let lead_own = lead_own.filter(|own| {
            let on = own.account.as_ref().map(|account| &account.id);
            by_hand.as_ref().is_none_or(|chosen| on == Some(chosen))
        });
        let own = lead_own.filter(|_| as_lead && !private_home);
        if let Some(own) = own {
            let mut launch = own.clone();
            launch.cwd = cwd.to_path_buf();
            launch.resume_id = None;
            launch.permission_mode = lead.worker_mode();
            launch.model = model.or(launch.model);
            launch.effort = None;
            return Ok((launch, lead.launcher.clone()));
        }
        let launcher = {
            let published = self.published.locked();
            let launchers = &published.launchers;
            let offered: Vec<&Launcher> = launchers
                .iter()
                .filter(|launcher| launcher.agent.provider == provider)
                .collect();
            let by_hand = by_hand.map(|chosen| LauncherId::on(provider, &chosen).to_string());
            let wanted = by_hand.as_ref().or(lead.launcher.as_ref());
            let wanted = (offered.iter()).find(|launcher| Some(&launcher.id) == wanted);
            if by_hand.is_some() && wanted.is_none() && offered.len() > 1 {
                return Err(format!(
                    "The {provider} account the person chose cannot start an agent, and only they may start one on another"
                ));
            }
            if same && wanted.is_none() && offered.len() > 1 {
                return Err(format!(
                    "Sikemux cannot tell which {provider} account you run on, so it will not start one on another; pick another cli or ask the person to start it"
                ));
            }
            let launcher = wanted.or_else(|| offered.first()).ok_or_else(|| {
                let mut known: Vec<&str> = launchers
                    .iter()
                    .map(|launcher| launcher.agent.provider.as_str())
                    .collect();
                known.sort_unstable();
                known.dedup();
                if known.is_empty() {
                    NO_AGENTS.to_owned()
                } else {
                    format!(
                        "{provider} is not set up on this host, so ask the person to install it or sign in to it; Sikemux can start {}",
                        known.join(", ")
                    )
                }
            })?;
            (*launcher).clone()
        };
        if let Some(refusal) = launcher.refusal() {
            return Err(refusal);
        }
        let model = model.or_else(|| {
            lead_own
                .filter(|_| as_lead)
                .and_then(|own| own.model.clone())
        });
        let mut launch = self.chat_launch(
            &launcher,
            still_there(&launcher)?,
            cwd,
            route.as_deref(),
            model,
        )?;
        launch.permission_mode = lead.worker_mode();
        if let Some(own) = lead_own {
            launch.account = own.account.clone();
            launch.fallbacks = own.fallbacks.clone();
        }
        Ok((launch, Some(launcher.id)))
    }

    /// The account the person chose for `provider`'s agents, when only they choose among them.
    fn chosen_by_hand(&self, provider: &str) -> Option<String> {
        by_hand(provider).then(|| self.accounts().unwrap_or_default().chosen(provider))
    }

    /// How the board starts an agent of its own in `cwd`: with the launcher
    /// named, else the first the app offers that works, in that launcher's own
    /// permission mode. A launcher gone is to be picked again `picked_in`.
    /// Reads the Keychain, so it blocks.
    pub fn board_launch(
        &self,
        launcher: Option<&str>,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
        picked_in: &str,
    ) -> Result<(ChatLaunch, String), String> {
        let chosen = {
            let published = self.published.locked();
            let launchers = &published.launchers;
            let book = published.saved.accounts.clone().unwrap_or_default();
            let usable = |launcher: &&Launcher| {
                let kind = &launcher.agent.provider;
                launcher.refusal().is_none()
                    && (!by_hand(kind)
                        || launcher.id == LauncherId::on(kind, &book.chosen(kind)).to_string())
            };
            match launcher {
                Some(id) => launchers
                    .iter()
                    .find(|launcher| launcher.id == id)
                    .ok_or_else(|| {
                        format!("Sikemux can no longer start {id}; pick another agent {picked_in}")
                    })?,
                None => launchers.iter().find(usable).ok_or_else(|| {
                    (launchers.iter().find_map(Launcher::refusal))
                        .unwrap_or_else(|| NO_AGENTS.to_owned())
                })?,
            }
            .clone()
        };
        if let Some(refusal) = chosen.refusal() {
            return Err(refusal);
        }
        let launch = self.chat_launch(&chosen, still_there(&chosen)?, cwd, provider, model)?;
        Ok((launch, chosen.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_cli_worker_launch_does_not_inherit_leads_effort() {
        let workspaces = Workspaces::new(None, None);
        let lead_launch = ChatLaunch {
            agent_id: "lead-1".into(),
            provider: "claude".into(),
            program: PathBuf::from("/usr/bin/true"),
            cwd: PathBuf::from("/tmp/lead"),
            args: Vec::new(),
            env: BTreeMap::new(),
            mcp_servers: Vec::new(),
            resume_id: None,
            permission_mode: PermissionMode::Bypass,
            model: Some("claude-opus-5-5".into()),
            model_provider: None,
            effort: Some("max".into()),
            account: None,
            fallbacks: Vec::new(),
        };
        let lead = Lead {
            provider: "claude".into(),
            launch: Some(lead_launch),
            launcher: Some("claude".into()),
            permission_mode: Some(PermissionMode::Bypass),
        };
        let (worker, _) = workspaces
            .worker_launch(
                &lead,
                "claude",
                Path::new("/tmp/worker"),
                Some("claude-opus-4-7".into()),
                Paid::AsLead,
            )
            .expect("worker launch");
        assert_eq!(worker.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(worker.effort, None);
    }
}
