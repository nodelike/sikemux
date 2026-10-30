//! How agents an agent or Auto starts are launched: on whose account, in which mode, and which
//! launcher reviews.

use super::*;

/// How a lead runs, for an agent it starts to run the same way.
pub(crate) struct Lead {
    pub provider: String,
    /// Its launch, when it is a chat the core runs.
    pub launch: Option<ChatLaunch>,
    pub launcher: Option<String>,
    pub permission_mode: Option<PermissionMode>,
}

impl Lead {
    /// The permission mode a worker of the lead takes: the lead's own, or
    /// Normal when the lead's is unknown.
    pub(crate) fn worker_mode(&self) -> PermissionMode {
        self.permission_mode.unwrap_or_default()
    }
}

/// Who pays for a worker's turns.
pub(crate) enum Paid {
    /// Whoever pays for its lead's, when it runs the lead's agent.
    AsLead,
    /// The provider named, or the agent's own account.
    By(Option<String>),
}

/// The program the core last found for `launcher`, while it is still there.
pub(super) fn still_there(launcher: &Launcher) -> Result<PathBuf, String> {
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
    pub(crate) fn worker_launch(
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
        let own = lead_own.filter(|_| as_lead && !private_home);
        if let Some(own) = own {
            let mut launch = own.clone();
            launch.cwd = cwd.to_path_buf();
            launch.resume_id = None;
            launch.permission_mode = lead.worker_mode();
            launch.model = model.or(launch.model);
            return Ok((launch, lead.launcher.clone()));
        }
        let launcher = {
            let published = self.published.locked();
            let launchers = &published.launchers;
            let offered: Vec<&Launcher> = launchers
                .iter()
                .filter(|launcher| launcher.agent.provider == provider)
                .collect();
            let lead_own = offered
                .iter()
                .find(|launcher| Some(&launcher.id) == lead.launcher.as_ref());
            if same && lead_own.is_none() && offered.len() > 1 {
                return Err(format!(
                    "Sikemux cannot tell which {provider} account you run on, so it will not start one on another; pick another cli or ask the person to start it"
                ));
            }
            let launcher = lead_own.or_else(|| offered.first()).ok_or_else(|| {
                let mut known: Vec<&str> = launchers
                    .iter()
                    .map(|launcher| launcher.agent.provider.as_str())
                    .collect();
                known.sort_unstable();
                known.dedup();
                if known.is_empty() {
                    "Sikemux has not said which agents it can start; open Sikemux once so it can"
                        .to_owned()
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

    pub(crate) fn has_launchers(&self) -> bool {
        !self.published.locked().launchers.is_empty()
    }

    /// How Auto starts an agent of its own in `cwd`: with the launcher the
    /// project names, else the first the app offers that works, in that
    /// launcher's own permission mode. Reads the Keychain, so it blocks.
    pub(crate) fn auto_launch(
        &self,
        launcher: Option<&str>,
        cwd: &Path,
        provider: Option<&str>,
        model: Option<String>,
    ) -> Result<(ChatLaunch, String), String> {
        let chosen = {
            let published = self.published.locked();
            let launchers = &published.launchers;
            let usable = |launcher: &&Launcher| launcher.refusal().is_none();
            match launcher {
                Some(id) => launchers
                    .iter()
                    .find(|launcher| launcher.id == id)
                    .ok_or_else(|| {
                        format!("Sikemux can no longer start {id}; pick another agent in the board's Auto settings")
                    })?,
                None => launchers.iter().find(usable).ok_or(
                    "Sikemux has not said which agents it can start; open Sikemux once so it can",
                )?,
            }
            .clone()
        };
        if let Some(refusal) = chosen.refusal() {
            return Err(refusal);
        }
        let launch = self.chat_launch(&chosen, still_there(&chosen)?, cwd, provider, model)?;
        Ok((launch, chosen.id))
    }

    /// The CLI the app's launcher `id` runs, such as `claude`.
    pub(crate) fn launcher_cli(&self, id: &str) -> Option<String> {
        self.published
            .locked()
            .launchers
            .iter()
            .find(|launcher| launcher.id == id)
            .map(|launcher| launcher.agent.provider.clone())
    }

    /// The launcher a reviewer starts with: `chosen`, else the first signed
    /// in that runs another CLI than `builder`. None when there is none.
    pub(crate) fn reviewer_launcher(
        &self,
        chosen: Option<&str>,
        builder: Option<&str>,
    ) -> Result<Option<String>, String> {
        let published = self.published.locked();
        let other = |launcher: &&Launcher| builder != Some(launcher.agent.provider.as_str());
        let Some(id) = chosen else {
            return Ok(published
                .launchers
                .iter()
                .filter(other)
                .find(|launcher| matches!(launcher.status, Some(AgentStatus::Ready { .. })))
                .map(|launcher| launcher.id.clone()));
        };
        let launcher = published
            .launchers
            .iter()
            .find(|launcher| launcher.id == id)
            .ok_or_else(|| {
                format!("Sikemux can no longer start the reviewer {id}; pick another in the board's Auto settings")
            })?;
        if !other(&launcher) {
            return Err(format!(
                "The reviewer chosen in Auto's settings, {}, runs the same CLI as the card's agent; pick one on another",
                launcher.label
            ));
        }
        Ok(Some(launcher.id.clone()))
    }
}
