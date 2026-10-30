// Signing workspaces in and out with a token from a Slack app, and saying who
// the app is talking to Slack as.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_slack_views::{Listed, Signed};

use crate::client::{self, Session};
use crate::config::{self, SlackConfig, Workspace};
use crate::error::{SlackError, SlackResult};

/// Who a token belongs to, as `auth.test` says.
pub fn workspace_of(answer: &Value) -> SlackResult<Workspace> {
    let text = |key: &str| answer.get(key).and_then(Value::as_str).map(str::to_string);
    let url = text("url").unwrap_or_default();
    let domain = url
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_ascii_lowercase();
    Ok(Workspace {
        id: text("team_id")
            .ok_or_else(|| SlackError::Response("Slack did not name the workspace".into()))?,
        name: text("team").unwrap_or_else(|| domain.clone()),
        domain,
        user_id: text("user_id").ok_or_else(|| {
            SlackError::Response("Slack did not say who the token belongs to".into())
        })?,
        user: text("user").unwrap_or_default(),
    })
}

pub type Status = SignInStatus<Signed>;

/// Every workspace signed in, and whether the default one's token still works.
pub async fn status(data_dir: &Path) -> Status {
    let config = match config::load(data_dir) {
        Ok(config) => config,
        Err(error) => return Status::checked(false, Signed::default(), &Err::<(), _>(error)),
    };
    let workspaces = config
        .workspaces
        .iter()
        .map(|workspace| Listed {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            domain: workspace.domain.clone(),
            user: workspace.user.clone(),
            user_id: workspace.user_id.clone(),
            is_default: config.default.as_deref() == Some(workspace.id.as_str()),
        })
        .collect();
    let check = match Session::of(data_dir, None).await {
        Ok(session) => session.get("auth.test", &[]).await,
        Err(error) => Err(error),
    };
    Status::checked(!config.workspaces.is_empty(), Signed { workspaces }, &check)
}

#[derive(Deserialize)]
pub struct TokenSignIn {
    pub token: String,
}

/// A user token, `xoxp-…`, posts as the person; a bot token, `xoxb-…`, as the app.
pub fn token_kind_ok(token: &str) -> bool {
    token.starts_with("xoxp-") || token.starts_with("xoxb-") || token.starts_with("xoxe.xoxp-")
}

/// Adds the workspace the token belongs to, and says which one it is.
pub async fn sign_in(data_dir: &Path, input: TokenSignIn) -> SlackResult<String> {
    let token = input.token.trim().to_string();
    if token.is_empty() {
        return Err(SlackError::BadArg("no token was given".into()));
    }
    if !token_kind_ok(&token) {
        return Err(SlackError::BadArg(
            "that is not a Slack token; use the User OAuth Token (xoxp-…) from the app's OAuth & Permissions page".into(),
        ));
    }
    let workspace = workspace_of(&client::get_with(&token, "auth.test", &[]).await?)?;
    let id = workspace.id.clone();
    store::update(data_dir, move |config: &mut SlackConfig| {
        config::KEYCHAIN.write(&workspace.id, &token)?;
        config.upsert(workspace);
        Ok(())
    })
    .await?;
    client::forget(&id);
    Ok(id)
}

#[derive(Deserialize)]
pub struct WorkspaceRef {
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Signs out the workspace named, or the default one.
pub async fn sign_out(data_dir: &Path, input: WorkspaceRef) -> SlackResult<()> {
    let removed = store::update(data_dir, move |config: &mut SlackConfig| {
        let Some(id) = config
            .account(input.workspace.as_deref())
            .map(|workspace| workspace.id.clone())
        else {
            return Ok(None);
        };
        let removed = config.remove(&id);
        if let Some(workspace) = &removed {
            config::KEYCHAIN.delete(&workspace.id)?;
        }
        Ok(removed)
    })
    .await?;
    if let Some(workspace) = removed {
        client::forget(&workspace.id);
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct DefaultChoice {
    pub id: String,
}

pub async fn set_default(data_dir: &Path, input: DefaultChoice) -> SlackResult<()> {
    store::update(data_dir, move |config: &mut SlackConfig| {
        config.set_default(&input.id)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_names_its_workspace_and_person() -> SlackResult<()> {
        let workspace = workspace_of(&json!({
            "ok": true, "url": "https://acme.slack.com/", "team": "Acme", "user": "ankit", "team_id": "T024BE7LD", "user_id": "U01AB"
        }))?;
        assert_eq!(workspace.id, "T024BE7LD");
        assert_eq!(workspace.domain, "acme.slack.com");
        assert_eq!(
            (workspace.user.as_str(), workspace.user_id.as_str()),
            ("ankit", "U01AB")
        );
        assert!(workspace_of(&json!({ "ok": true })).is_err());
        Ok(())
    }

    #[test]
    fn only_a_slack_token_is_taken() {
        assert!(token_kind_ok("xoxp-1-2-3"));
        assert!(token_kind_ok("xoxb-1-2"));
        assert!(!token_kind_ok("glpat-abc"));
        assert!(!token_kind_ok("xapp-1-A0"));
    }
}
