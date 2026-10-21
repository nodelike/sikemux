// Deployment protection: whether a project's deployments ask visitors to sign in to
// Vercel or for a password, and a link that lets someone in without either. The link
// carries the project's bypass secret, so it goes only to the person's clipboard,
// never to an agent.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::{segment, ProjectRef};

/// Which deployments a protection covers, in the dashboard's words.
fn covers(kind: &Value) -> Value {
    let words = match kind.as_str() {
        Some("preview") => "Preview deployments",
        Some("prod_deployment_urls_and_all_previews") => "Previews and production's own URLs",
        Some("all") => "All deployments",
        Some("all_except_custom_domains") => "All but custom domains",
        Some(_) => "Some deployments",
        None => return Value::Null,
    };
    json!(words)
}

/// The project's bypass secret for sharing, if it has one.
fn bypass_secret(project: &Value) -> Option<String> {
    at(project, "/protectionBypass")
        .as_object()?
        .iter()
        .find(|(_, about)| at(about, "/scope").as_str() == Some("automation-bypass"))
        .map(|(secret, _)| secret.clone())
}

pub fn protection_of(project: &Value) -> Value {
    json!({
        "vercelAuthentication": covers(at(project, "/ssoProtection/deploymentType")),
        "password": covers(at(project, "/passwordProtection/deploymentType")),
        "canShare": bypass_secret(project).is_some(),
    })
}

pub async fn protection(data_dir: &Path, input: ProjectRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    Ok(protection_of(&read))
}

#[derive(Deserialize)]
pub struct Share {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    /// The deployment's address, such as web-abc123.vercel.app.
    pub url: String,
}

/// The deployment's address with the bypass secret, set to leave a cookie so the visitor's next pages open too.
pub fn share_link(url: &str, secret: &str) -> VercelResult<String> {
    let host = url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'));
    if !plain {
        return Err(VercelError::BadArg(format!(
            "`{url}` is not a deployment address"
        )));
    }
    Ok(format!(
        "https://{host}/?x-vercel-protection-bypass={secret}&x-vercel-set-bypass-cookie=samesitenone"
    ))
}

/// A link to the deployment that opens without signing in, making the project's bypass secret first if it has none.
pub async fn share(data_dir: &Path, input: Share) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    let secret = match bypass_secret(&read) {
        Some(secret) => secret,
        None => {
            let made = session
                .send(
                    Method::PATCH,
                    &format!("/v1/projects/{project}/protection-bypass"),
                    &[],
                    Some(json!({ "generate": {} })),
                )
                .await?;
            bypass_secret(&made)
                .ok_or_else(|| VercelError::Response("Vercel made no bypass secret".into()))?
        }
    };
    let link = share_link(&input.url, &secret)?;
    Ok(json!({ "link": link, "project": text(&read, "/name") }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_reads_in_the_dashboards_words_without_the_secret() {
        let project = json!({
            "ssoProtection": { "deploymentType": "prod_deployment_urls_and_all_previews" },
            "passwordProtection": null,
            "protectionBypass": { "s3cr3t": { "createdAt": 1, "scope": "automation-bypass" } }
        });
        let read = protection_of(&project);
        assert_eq!(
            read["vercelAuthentication"],
            "Previews and production's own URLs"
        );
        assert_eq!(read["password"], Value::Null);
        assert_eq!(read["canShare"], true);
        assert!(!read.to_string().contains("s3cr3t"));
        assert_eq!(protection_of(&json!({}))["canShare"], false);
    }

    #[test]
    fn only_a_sharing_secret_is_used() {
        let other = json!({ "protectionBypass": { "abc": { "scope": "shareable-link" } } });
        assert_eq!(bypass_secret(&other), None);
        let sharing = json!({ "protectionBypass": { "abc": { "scope": "shareable-link" }, "def": { "scope": "automation-bypass" } } });
        assert_eq!(bypass_secret(&sharing).as_deref(), Some("def"));
    }

    #[test]
    fn a_share_link_points_at_the_deployment_and_nowhere_else() {
        assert_eq!(
            share_link("https://web-abc123.vercel.app/", "s3cr3t").unwrap(),
            "https://web-abc123.vercel.app/?x-vercel-protection-bypass=s3cr3t&x-vercel-set-bypass-cookie=samesitenone"
        );
        assert!(share_link("evil.example/path?x=", "s").is_err());
        assert!(share_link("", "s").is_err());
    }
}
