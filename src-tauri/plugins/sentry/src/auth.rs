// Signing in with a personal auth token, choosing the organisation and project,
// and saying whether the sign-in still works.

use std::path::Path;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{self, text, Session};
use crate::config::{self, Settings};
use crate::error::{SentryError, SentryResult};

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub slug: String,
    pub name: String,
}

pub fn organizations_of(list: &Value) -> Vec<Organization> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|org| {
            Some(Organization {
                slug: text(org, "/slug")?,
                name: text(org, "/name").unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub platform: Option<String>,
}

pub fn projects_of(list: &Value) -> Vec<Project> {
    let mut projects: Vec<Project> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|project| {
            Some(Project {
                id: text(project, "/id")?,
                slug: text(project, "/slug")?,
                name: text(project, "/name").unwrap_or_default(),
                platform: text(project, "/platform"),
            })
        })
        .collect();
    projects.sort_by(|a, b| a.slug.cmp(&b.slug));
    projects
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
    pub url: String,
    pub org: String,
    pub org_name: String,
    pub project: Option<String>,
    pub user: String,
}

pub async fn status(data_dir: &Path) -> Status {
    let settings = config::load(data_dir);
    let base = |ok: bool, auth_failed: bool, message: Option<String>| Status {
        configured: settings.is_signed_in(),
        ok,
        auth_failed,
        message,
        url: settings.url.clone(),
        org: settings.org.clone(),
        org_name: settings.org_name.clone(),
        project: settings.project.clone(),
        user: settings.user.clone(),
    };
    let session = match Session::open(data_dir).await {
        Ok(session) => session,
        Err(SentryError::Unconfigured) => return base(false, false, None),
        Err(error) => return base(false, false, Some(error.to_string())),
    };
    let path = format!("/organizations/{}/", session.settings.org);
    match session.get(&path, &[]).await {
        Ok(_) => base(true, false, None),
        Err(error @ SentryError::Auth(_)) => base(false, true, Some(error.to_string())),
        Err(error @ SentryError::RateLimited) => base(true, false, Some(error.to_string())),
        Err(error) => base(false, false, Some(error.to_string())),
    }
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
    #[serde(default)]
    pub url: String,
}

/// Turns away tokens that can't read issues before anything is sent.
pub fn check_token(token: &str) -> SentryResult<()> {
    if token.is_empty() {
        return Err(SentryError::BadArg("no auth token was given".into()));
    }
    if token.starts_with("sntrys_") {
        return Err(SentryError::BadArg(
            "that is an organisation token, which can only upload releases and source maps; make a personal token in Sentry under User Settings → Personal Tokens".into(),
        ));
    }
    if token.starts_with("https://") && token.contains("@") {
        return Err(SentryError::BadArg(
            "that is a DSN, which apps send errors with; paste a personal auth token instead"
                .into(),
        ));
    }
    Ok(())
}

/// Who signed in, as the organisation's member list says of `me`.
fn member_of(body: &Value) -> (String, String) {
    let id = text(body, "/user/id").unwrap_or_default();
    let name = text(body, "/user/name")
        .or_else(|| text(body, "/name"))
        .or_else(|| text(body, "/email"))
        .unwrap_or_default();
    (id, name)
}

/// Checks the token against Sentry, keeps it in the Keychain, and signs in to its first organisation.
pub async fn sign_in(data_dir: &Path, input: TokenSignIn) -> SentryResult<()> {
    let token = input.token.trim().to_string();
    check_token(&token)?;
    let url = config::base_url(&input.url)?;
    let organizations = organizations_of(
        &client::send_with(&url, &token, Method::GET, "/organizations/", &[], None)
            .await?
            .body,
    );
    let org = organizations.into_iter().next().ok_or_else(|| {
        SentryError::Forbidden(
            "this token can see no organisation; give it the org:read scope".into(),
        )
    })?;
    let me = client::send_with(
        &url,
        &token,
        Method::GET,
        &format!("/organizations/{}/members/me/", org.slug),
        &[],
        None,
    )
    .await
    .map(|page| member_of(&page.body))
    .unwrap_or_default();
    let settings = Settings {
        url,
        org: org.slug,
        org_name: org.name,
        project: None,
        user_id: me.0,
        user: me.1,
    };
    let dir = data_dir.to_path_buf();
    config::blocking(move || {
        config::token_write(&token)?;
        config::save(&dir, &settings)
    })
    .await?;
    client::forget_token();
    Ok(())
}

pub async fn sign_out(data_dir: &Path) -> SentryResult<()> {
    let dir = data_dir.to_path_buf();
    config::blocking(move || {
        config::token_delete()?;
        config::forget(&dir)
    })
    .await?;
    client::forget_token();
    Ok(())
}

pub async fn organizations(data_dir: &Path) -> SentryResult<Vec<Organization>> {
    let session = Session::open(data_dir).await?;
    Ok(organizations_of(
        &session.get("/organizations/", &[]).await?,
    ))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Choose {
    pub org: Option<String>,
    /// A project slug; an empty one means every project.
    pub project: Option<String>,
}

/// Switches the organisation, or the project shown first, and keeps the choice.
pub async fn choose(data_dir: &Path, input: Choose) -> SentryResult<Status> {
    let session = Session::open(data_dir).await?;
    let mut settings = session.settings.clone();
    if let Some(org) = input.org.filter(|org| *org != settings.org) {
        let org = client::slug("organisation", &org)?.to_string();
        let found = session.get(&format!("/organizations/{org}/"), &[]).await?;
        settings.org_name = text(&found, "/name").unwrap_or_else(|| org.clone());
        settings.org = org;
        settings.project = None;
        let me = session
            .get(&format!("/organizations/{}/members/me/", settings.org), &[])
            .await
            .map(|body| member_of(&body))
            .unwrap_or_default();
        settings.user_id = me.0;
        settings.user = me.1;
    }
    if let Some(project) = input.project {
        settings.project = match project.as_str() {
            "" => None,
            chosen => Some(client::slug("project", chosen)?.to_string()),
        };
    }
    let dir = data_dir.to_path_buf();
    let kept = settings.clone();
    config::blocking(move || config::save(&dir, &kept)).await?;
    Ok(status(data_dir).await)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct InOrg {
    pub org: Option<String>,
}

pub async fn projects(data_dir: &Path, input: InOrg) -> SentryResult<Vec<Project>> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    Ok(projects_of(
        &session
            .get(
                &format!("/organizations/{org}/projects/"),
                &[("per_page", "100".into())],
            )
            .await?,
    ))
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// What Sentry takes to assign an issue, such as user:42.
    pub actor: String,
    pub name: String,
    pub email: Option<String>,
    pub is_me: bool,
}

pub fn members_of(list: &Value, me: &str) -> Vec<Member> {
    let mut members: Vec<Member> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|member| !at_pending(member))
        .filter_map(|member| {
            let id = text(member, "/user/id")?;
            Some(Member {
                is_me: id == me,
                actor: format!("user:{id}"),
                name: text(member, "/user/name")
                    .or_else(|| text(member, "/name"))
                    .unwrap_or_default(),
                email: text(member, "/user/email").or_else(|| text(member, "/email")),
            })
        })
        .collect();
    members.sort_by(|a, b| b.is_me.cmp(&a.is_me).then_with(|| a.name.cmp(&b.name)));
    members
}

fn at_pending(member: &Value) -> bool {
    client::at(member, "/pending").as_bool().unwrap_or(false)
        || client::at(member, "/user").is_null()
}

pub async fn members(data_dir: &Path, input: InOrg) -> SentryResult<Vec<Member>> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let list = session
        .get(
            &format!("/organizations/{org}/members/"),
            &[("per_page", "100".into())],
        )
        .await?;
    Ok(members_of(&list, &session.settings.user_id))
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct InProject {
    pub org: Option<String>,
    pub project: Option<String>,
}

/// The environments a project has sent events from, busiest first as Sentry lists them.
pub async fn environments(data_dir: &Path, input: InProject) -> SentryResult<Vec<String>> {
    let session = Session::open(data_dir).await?;
    let org = client::slug("organisation", session.org(input.org.as_deref()))?;
    let path = match input
        .project
        .as_deref()
        .filter(|project| !project.is_empty())
    {
        Some(project) => format!(
            "/projects/{org}/{}/environments/",
            client::slug("project", project)?
        ),
        None => format!("/organizations/{org}/environments/"),
    };
    let list = session
        .get(&path, &[("visibility", "visible".into())])
        .await?;
    Ok(list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|environment| text(environment, "/name"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens_that_cannot_read_issues_are_turned_away() {
        assert!(check_token("sntryu_0123abcd").is_ok());
        assert!(check_token("0123456789abcdef0123456789abcdef").is_ok());
        assert!(matches!(check_token(""), Err(SentryError::BadArg(_))));
        assert!(
            matches!(check_token("sntrys_eyJpYXQiOjE"), Err(SentryError::BadArg(message)) if message.contains("Personal Tokens"))
        );
        assert!(
            matches!(check_token("https://abc123@o1.ingest.sentry.io/42"), Err(SentryError::BadArg(message)) if message.contains("DSN"))
        );
    }

    #[test]
    fn organizations_projects_and_members_are_read_from_sentrys_lists() {
        let orgs =
            organizations_of(&json!([{ "slug": "acme", "name": "Acme" }, { "name": "no slug" }]));
        assert_eq!(
            orgs,
            [Organization {
                slug: "acme".into(),
                name: "Acme".into()
            }]
        );
        let projects = projects_of(&json!([
            { "id": "2", "slug": "web", "name": "Web", "platform": "javascript-nextjs" },
            { "id": "1", "slug": "api", "name": "API", "platform": "python" }
        ]));
        assert_eq!(projects[0].slug, "api");
        let members = members_of(
            &json!([
                { "user": { "id": "7", "name": "Zoe", "email": "zoe@acme.dev" } },
                { "user": { "id": "42", "name": "Ankit", "email": "ankit@acme.dev" } },
                { "email": "new@acme.dev", "pending": true, "user": null }
            ]),
            "42",
        );
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].actor, "user:42");
        assert!(members[0].is_me);
    }

    #[test]
    fn who_signed_in_is_read_from_the_member_record() {
        assert_eq!(
            member_of(&json!({ "user": { "id": "42", "name": "Ankit" }, "email": "a@acme.dev" })),
            ("42".to_string(), "Ankit".to_string())
        );
    }
}
