use std::path::PathBuf;

use serde::Deserialize;
use serde_json::json;
use sikemux_core::agent_accounts::status::AgentStatus;
use sikemux_core::agent_catalog::AGENTS;
use sikemux_core::client::CoreClient;
use sikemux_core::launch::discovery;
use tauri::State;

use super::config::{configured_default_effort, configured_default_model};
use super::{AgentInfo, AgentProfileRequest};
use crate::error::AppResult;
use crate::pty::{core_error, PtyManager};

/// Where the core found an account's agent program, what failed on the way,
/// and whether it is signed in.
#[derive(Deserialize)]
struct Availability {
    program: Option<PathBuf>,
    failures: Vec<String>,
    status: AgentStatus,
}

/// The agent CLIs installed for the person, as the core finds them, which is
/// where they run.
#[tauri::command]
pub async fn available_agents(
    manager: State<'_, PtyManager>,
    profiles: Vec<AgentProfileRequest>,
) -> AppResult<Vec<AgentInfo>> {
    let client = manager.client().await?;
    let listed = AGENTS.iter().filter_map(|def| {
        let profile = profiles
            .iter()
            .find(|profile| profile.kind.as_str() == def.kind);
        let explicit = profile.and_then(|profile| profile.executable_path.as_deref());
        (profile.is_some() || !discovery::candidates(def, explicit).is_empty())
            .then_some((def, profile, explicit))
    });
    let client = &client;
    futures::future::try_join_all(listed.map(|(def, profile, explicit)| async move {
        let config_path = profile.and_then(|profile| profile.config_path.clone());
        let Availability {
            program,
            failures,
            status,
        } = availability(client, def.kind, config_path.clone()).await?;
        let warning = program.as_ref().and_then(|path| {
            (!failures.is_empty())
                .then(|| format!("Skipped {}; using {}", failures.join("; "), path.display()))
        });
        let error = program.is_none().then(|| {
            if failures.is_empty() {
                format!("{} executable was not found", def.label)
            } else {
                failures.join("; ")
            }
        });
        Ok(AgentInfo {
            kind: def.kind,
            label: def.label,
            command: program
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| explicit.map(str::to_string))
                .unwrap_or_else(|| def.command.to_string()),
            available: program.is_some(),
            error,
            warning,
            profile_id: profile.and_then(|profile| profile.profile_id.clone()),
            default_model: configured_default_model(def.kind, config_path.as_deref()),
            default_effort: configured_default_effort(def.kind, config_path.as_deref()),
            config_path,
            status,
        })
    }))
    .await
}

async fn availability(
    client: &CoreClient,
    provider: &str,
    config_path: Option<String>,
) -> AppResult<Availability> {
    let params = json!({ "provider": provider, "configPath": config_path });
    let answer = client
        .call("agentAvailability", params)
        .await
        .map_err(core_error)?;
    Ok(serde_json::from_value(answer)?)
}
