// Recent releases of a project: when each shipped, the issues it brought, its last
// deploy, and the share of sessions and users that did not crash.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{self, at, number, text, Session};
use crate::error::SentryResult;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    /// The version as Sentry shortens it, such as 4.13.0 (a1b2c3d).
    pub short: String,
    pub created: Option<String>,
    pub new_issues: u64,
    pub commits: u64,
    pub deployed_to: Option<String>,
    pub deployed_at: Option<String>,
    /// Percent of sessions that did not crash, when the SDK sends sessions.
    pub crash_free_sessions: Option<f64>,
    pub crash_free_users: Option<f64>,
    pub sessions: u64,
}

pub fn release_of(item: &Value, project: Option<&str>) -> Option<Release> {
    let version = text(item, "/version")?;
    let health = at(item, "/projects")
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| project.is_none_or(|slug| text(entry, "/slug").as_deref() == Some(slug)))
        .map(|entry| at(entry, "/healthData"))
        .unwrap_or(&Value::Null);
    Some(Release {
        short: text(item, "/shortVersion")
            .or_else(|| text(item, "/versionInfo/description"))
            .unwrap_or_else(|| version.clone()),
        version,
        created: text(item, "/dateCreated"),
        new_issues: number(item, "/newGroups"),
        commits: number(item, "/commitCount"),
        deployed_to: text(item, "/lastDeploy/environment"),
        deployed_at: text(item, "/lastDeploy/dateFinished"),
        crash_free_sessions: at(health, "/crashFreeSessions").as_f64(),
        crash_free_users: at(health, "/crashFreeUsers").as_f64(),
        sessions: number(health, "/totalSessions24h").max(number(health, "/totalSessions")),
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListReleases {
    pub org: Option<String>,
    /// The project's slug.
    pub project: Option<String>,
    /// Its numeric id, which Sentry needs to say how healthy each release is; looked up when not given.
    pub project_id: Option<String>,
    pub limit: Option<u64>,
}

pub async fn list(data_dir: &Path, input: ListReleases) -> SentryResult<Vec<Release>> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let mut query = vec![
        (
            "per_page",
            input.limit.unwrap_or(20).clamp(1, 100).to_string(),
        ),
        ("summaryStatsPeriod", "24h".to_string()),
    ];
    let slug = input.project.as_deref().filter(|p| !p.is_empty());
    let id = match (input.project_id.as_deref().filter(|p| !p.is_empty()), slug) {
        (Some(id), _) => Some(client::slug("project", id)?.to_string()),
        (None, Some(slug)) => Some(client::project_id(&session, org, slug).await?),
        (None, None) => None,
    };
    if let Some(id) = id {
        query.push(("project", id));
        query.push(("health", "1".to_string()));
    }
    let list = session
        .get(&format!("/organizations/{org}/releases/"), &query)
        .await?;
    Ok(list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| release_of(item, slug))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_release_reads_its_new_issues_deploy_and_health() {
        let item = json!({
            "version": "web@4.13.0+a1b2c3d", "shortVersion": "4.13.0 (a1b2c3d)", "dateCreated": "2026-10-19T09:00:00Z",
            "newGroups": 3, "commitCount": 14,
            "lastDeploy": { "environment": "production", "dateFinished": "2026-10-19T09:20:00Z" },
            "projects": [
                { "slug": "api", "healthData": { "crashFreeSessions": 99.9 } },
                { "slug": "web", "healthData": { "crashFreeSessions": 97.42, "crashFreeUsers": 98.1, "totalSessions24h": 18233 } }
            ]
        });
        let release = release_of(&item, Some("web")).expect("reads");
        assert_eq!(release.short, "4.13.0 (a1b2c3d)");
        assert_eq!((release.new_issues, release.commits), (3, 14));
        assert_eq!(release.deployed_to.as_deref(), Some("production"));
        assert_eq!(release.crash_free_sessions, Some(97.42));
        assert_eq!(release.sessions, 18233);
        let bare = release_of(&json!({ "version": "1.0.0" }), None).expect("reads");
        assert_eq!(
            (bare.short.as_str(), bare.crash_free_sessions),
            ("1.0.0", None)
        );
    }
}
