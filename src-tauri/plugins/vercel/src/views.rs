//! What the Vercel pane shows, kept by the core and read again on the pace the manifest
//! gives: the team's projects, a project's deployments, domains and variables, and a
//! deployment's build log with the text an agent is handed of it.

use std::path::Path;

use serde::Deserialize;
use sikemux_plugin_api::vercel::{Deployments, Domains, Log, OpenLog, Projects, Target, Variables};

use crate::deployments::{self, LogRef};
use crate::error::VercelResult;
use crate::projects::{self, Listing, ProjectRef};

/// How many deployments a project's list shows.
const DEPLOYMENTS_SHOWN: u64 = 50;
/// How much of a build log the pane keeps.
const LOG_LINES_SHOWN: usize = 2000;
/// How much of a build log an agent is handed.
const LOG_LINES_FOR_AGENT: usize = 120;

#[derive(Deserialize)]
pub struct TeamRef {
    #[serde(default)]
    pub team: Option<String>,
}

#[derive(Deserialize)]
pub struct DeploymentRef {
    pub id: String,
    #[serde(default)]
    pub team: Option<String>,
}

pub async fn projects_view(data_dir: &Path, input: TeamRef) -> VercelResult<Projects> {
    let listing = Listing {
        query: None,
        team: input.team,
    };
    Ok(Projects {
        projects: projects::projects(data_dir, listing).await?,
    })
}

pub async fn deployments_view(data_dir: &Path, input: ProjectRef) -> VercelResult<Deployments> {
    let listing = deployments::Listing {
        project: Some(input.project),
        target: None,
        state: None,
        branch: None,
        limit: Some(DEPLOYMENTS_SHOWN),
        team: input.team,
    };
    Ok(Deployments {
        deployments: deployments::deployments(data_dir, listing).await?,
    })
}

pub async fn log_view(data_dir: &Path, input: DeploymentRef) -> VercelResult<OpenLog> {
    let log = deployments::logs(
        data_dir,
        LogRef {
            id: input.id,
            tail: Some(LOG_LINES_SHOWN),
            team: input.team,
        },
    )
    .await?;
    Ok(OpenLog {
        for_agent: for_agent(&log),
        log,
    })
}

pub async fn domains_view(data_dir: &Path, input: ProjectRef) -> VercelResult<Domains> {
    Ok(Domains {
        domains: projects::domains(data_dir, input).await?,
    })
}

pub async fn env_view(data_dir: &Path, input: ProjectRef) -> VercelResult<Variables> {
    Ok(Variables {
        variables: projects::env(data_dir, input).await?,
    })
}

/// A deployment as an agent reads it: what it is, where it came from, and the end of its build log.
fn for_agent(log: &Log) -> String {
    let deployment = &log.deployment;
    let mut parts = Vec::new();
    let url = deployment
        .url
        .as_ref()
        .map_or_else(String::new, |url| format!(" — {url}"));
    parts.push(format!(
        "Vercel deployment {} of {}{url}",
        deployment.id, deployment.project
    ));
    let target = match deployment.target {
        Target::Production => "production",
        Target::Preview => "preview",
    };
    let mut facts = vec![
        format!("State: {}", deployment.state),
        format!("Target: {target}"),
    ];
    if let Some(branch) = &deployment.branch {
        facts.push(format!("Branch: {branch}"));
    }
    if let Some(commit) = &deployment.commit {
        let message = deployment
            .message
            .as_ref()
            .map_or_else(String::new, |message| format!(" — {message}"));
        facts.push(format!("Commit: {commit}{message}"));
    }
    if let Some(author) = &deployment.author {
        facts.push(format!("By: {author}"));
    }
    if let Some(error) = &deployment.error {
        facts.push(format!("Error: {error}"));
    }
    parts.push(facts.join("\n"));
    let tail = log.lines.len().saturating_sub(LOG_LINES_FOR_AGENT);
    let lines: Vec<&str> = log
        .lines
        .iter()
        .skip(tail)
        .map(|line| line.text.as_str())
        .collect();
    if !lines.is_empty() {
        parts.push(format!(
            "End of the build log:\n\n```\n{}\n```",
            lines.join("\n")
        ));
    }
    parts.push(format!(
        "To read more, use vercel_logs or vercel_deployment with id {}.",
        deployment.id
    ));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use sikemux_plugin_api::vercel::{Deployment, LogLine};

    use super::*;

    #[test]
    fn a_failed_deployment_says_what_it_is_its_error_and_where_to_read_more() {
        let log = Log {
            deployment: Deployment {
                id: "dpl_bad".into(),
                project: "web".into(),
                project_id: Some("prj_1".into()),
                url: Some("https://web-bad.vercel.app".into()),
                state: "ERROR".into(),
                target: Target::Preview,
                created_at: 1,
                ready_at: None,
                build_ms: Some(45_000),
                branch: Some("fix-nav".into()),
                commit: Some("abc1234def".into()),
                message: Some("Fix the nav".into()),
                author: Some("Ankit".into()),
                redeploy_of: None,
                inspector_url: None,
                aliases: None,
                error: Some("Command \"npm run build\" exited with 1".into()),
            },
            lines: vec![LogLine {
                at: 2,
                kind: "stderr".into(),
                text: "Type error: nav is not defined".into(),
            }],
            skipped: 0,
        };
        let text = for_agent(&log);
        assert!(text.contains("Vercel deployment dpl_bad of web — https://web-bad.vercel.app"));
        assert!(text.contains("Error: Command \"npm run build\" exited with 1"));
        assert!(text.contains("vercel_logs or vercel_deployment with id dpl_bad"));
    }
}
