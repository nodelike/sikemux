use std::future::Future;
use std::sync::Arc;

use serde_json::json;
use sikemux_core::agent_accounts::status::AgentStatus;
use sikemux_core::agent_catalog::AGENTS;
use sikemux_core::client::CoreClient;
use sikemux_core::launch::discovery;
use tauri::State;

use super::config::{configured_default_effort, configured_default_model};
use super::{AgentInfo, AgentProfileRequest};
use crate::error::AppResult;
use crate::pty::PtyManager;

/// Agent CLIs that are installed for the current user. The app's PATH is fixed
/// from the login shell during boot, so this matches what spawned PTYs can run.
/// Whether each is signed in comes from the core, which keeps the answers.
#[tauri::command]
pub async fn available_agents(
    manager: State<'_, PtyManager>,
    profiles: Vec<AgentProfileRequest>,
) -> AppResult<Vec<AgentInfo>> {
    let client = manager.client().await.ok();
    Ok(catalog(&profiles, |provider, config_path| {
        signed_in(client.clone(), provider, config_path)
    })
    .await)
}

async fn signed_in(
    client: Option<Arc<CoreClient>>,
    provider: &'static str,
    config_path: Option<String>,
) -> AgentStatus {
    let Some(client) = client else {
        return AgentStatus::Unknown;
    };
    let params = json!({ "provider": provider, "configPath": config_path });
    crate::agent_names::call(&client, "agentStatus".into(), params)
        .await
        .ok()
        .and_then(|status| serde_json::from_value(status).ok())
        .unwrap_or(AgentStatus::Unknown)
}

async fn catalog<F: Future<Output = AgentStatus>>(
    profiles: &[AgentProfileRequest],
    signed_in: impl Fn(&'static str, Option<String>) -> F,
) -> Vec<AgentInfo> {
    let available = futures::future::join_all(AGENTS.iter().map(|def| async {
        let profile = profiles
            .iter()
            .find(|profile| profile.kind.as_str() == def.kind);
        let explicit = profile.and_then(|profile| profile.executable_path.as_deref());
        let candidates = discovery::candidates(def, explicit);
        if candidates.is_empty() && profile.is_none() {
            return None;
        }

        let (resolved, failures) = discovery::first_healthy(def.kind, candidates).await;
        let config_path = profile.and_then(|profile| profile.config_path.clone());
        let warning = resolved.as_ref().and_then(|path| {
            (!failures.is_empty())
                .then(|| format!("Skipped {}; using {}", failures.join("; "), path.display()))
        });
        let error = resolved.is_none().then(|| {
            if failures.is_empty() {
                format!("{} executable was not found", def.label)
            } else {
                failures.join("; ")
            }
        });
        let status = match &resolved {
            Some(_) => signed_in(def.kind, config_path.clone()).await,
            None if failures.is_empty() => AgentStatus::Missing,
            None => AgentStatus::Broken {
                reason: failures.join("; "),
            },
        };
        Some(AgentInfo {
            kind: def.kind,
            label: def.label,
            command: resolved
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned())
                .or_else(|| explicit.map(str::to_string))
                .unwrap_or_else(|| def.command.to_string()),
            available: resolved.is_some(),
            error,
            warning,
            profile_id: profile.and_then(|profile| profile.profile_id.clone()),
            config_path: config_path.clone(),
            default_model: configured_default_model(def.kind, config_path.as_deref()),
            default_effort: configured_default_effort(def.kind, config_path.as_deref()),
            status,
        })
    }))
    .await;
    available.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::{AgentProfileRequest, AgentStatus};

    #[cfg(unix)]
    #[tokio::test]
    async fn each_agent_reports_whether_it_is_found_working_and_signed_in() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("pi");
        fs::write(
            &pi,
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'pi 1.0' && exit 0\n[ \"$1 $2\" = 'auth check' ] && exit 1\nexit 64\n",
        )
        .unwrap();
        fs::set_permissions(&pi, fs::Permissions::from_mode(0o755)).unwrap();
        let broken = dir.path().join("omp");
        fs::write(
            &broken,
            "#!/bin/sh\nprintf 'launcher missing\\n' >&2\nexit 127\n",
        )
        .unwrap();
        fs::set_permissions(&broken, fs::Permissions::from_mode(0o755)).unwrap();
        let config = dir.path().join("profile").to_string_lossy().into_owned();
        let profiles: Vec<AgentProfileRequest> = serde_json::from_value(serde_json::json!([
            { "type": "pi", "executablePath": pi, "configPath": config },
            { "type": "omp", "executablePath": broken },
            { "type": "grok", "executablePath": dir.path().join("nowhere/grok") },
        ]))
        .unwrap();

        let agents = super::catalog(&profiles, |_, _| async { AgentStatus::Unknown }).await;
        let status_of = |kind: &str| {
            agents
                .iter()
                .find(|agent| agent.kind == kind)
                .map(|agent| agent.status.clone())
        };
        assert_eq!(status_of("pi"), Some(AgentStatus::Unknown));
        assert!(
            matches!(status_of("omp"), Some(AgentStatus::Broken { reason }) if reason.contains("launcher missing"))
        );
        assert!(
            matches!(
                status_of("grok"),
                Some(AgentStatus::Missing | AgentStatus::Broken { .. })
            ),
            "a profile whose binary is gone is not ready: {:?}",
            status_of("grok")
        );
    }
}
