// Signing workspaces in and out, through the browser or with a personal API key,
// and saying who the sign-in belongs to.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sikemux_plugin_api::StreamSink;

use crate::client::{self, at, Session};
use crate::config::{self, SignIn, Workspace};
use crate::error::{LinearError, LinearResult};
use crate::oauth;

const BROWSER_WAIT: Duration = Duration::from_secs(300);

const VIEWER: &str =
    "query Viewer { viewer { id name displayName email organization { id name urlKey } } }";

/// Who a sign-in belongs to, as `viewer` says.
pub fn workspace_of(data: &Value) -> LinearResult<Workspace> {
    let viewer = &at(data, "/viewer");
    let organization = &at(viewer, "/organization");
    let text = |value: &Value| value.as_str().map(str::to_string);
    Ok(Workspace {
        id: text(at(organization, "/id"))
            .ok_or_else(|| LinearError::Response("Linear did not name the workspace".into()))?,
        name: text(at(organization, "/name")).unwrap_or_default(),
        url_key: text(at(organization, "/urlKey")).unwrap_or_default(),
        user_id: text(at(viewer, "/id"))
            .ok_or_else(|| LinearError::Response("Linear did not say whose key this is".into()))?,
        user: text(at(viewer, "/displayName"))
            .or_else(|| text(at(viewer, "/name")))
            .unwrap_or_default(),
        sign_in: SignIn::Key,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub ok: bool,
    pub auth_failed: bool,
    pub message: Option<String>,
    pub workspaces: Vec<Listed>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub url_key: String,
    pub user: String,
    pub is_default: bool,
}

/// Every workspace signed in, and whether the default one's key still works.
pub async fn status(data_dir: &Path) -> Status {
    let config = config::load(data_dir);
    let workspaces = config
        .workspaces
        .iter()
        .map(|workspace| Listed {
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            url_key: workspace.url_key.clone(),
            user: workspace.user.clone(),
            is_default: config.default.as_deref() == Some(workspace.id.as_str()),
        })
        .collect();
    let base = |ok: bool, auth_failed: bool, message: Option<String>| Status {
        configured: !config.workspaces.is_empty(),
        ok,
        auth_failed,
        message,
        workspaces,
    };
    let session = match Session::of(data_dir, None).await {
        Ok(session) => session,
        Err(LinearError::Unconfigured) => return base(false, false, None),
        Err(error) => return base(false, false, Some(error.to_string())),
    };
    match session.query(VIEWER, json!({})).await {
        Ok(_) => base(true, false, None),
        Err(error @ LinearError::Auth(_)) => base(false, true, Some(error.to_string())),
        Err(error @ LinearError::RateLimited) => base(true, false, Some(error.to_string())),
        Err(error) => base(false, false, Some(error.to_string())),
    }
}

#[derive(Deserialize)]
pub struct KeySignIn {
    pub key: String,
}

/// Adds the workspace the key belongs to, and says which one it is.
pub async fn sign_in(data_dir: &Path, input: KeySignIn) -> LinearResult<String> {
    let key = input.key.trim().to_string();
    if key.is_empty() {
        return Err(LinearError::BadArg("no API key was given".into()));
    }
    if !key.starts_with("lin_api_") {
        return Err(LinearError::BadArg(
            "that is not a Linear API key; make one in Linear under Settings → Security & access → Personal API keys".into(),
        ));
    }
    let workspace = workspace_of(&client::query_with(&key, VIEWER, json!({})).await?)?;
    let id = workspace.id.clone();
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        config::keychain_write(&workspace, &key)?;
        let mut config = config::load(&data_dir);
        config.upsert(workspace);
        config::save(&data_dir, &config)
    }))
    .await?;
    client::forget(&id);
    Ok(id)
}

/// Opens Linear's sign-in in the browser, through the URL sent down the stream, and
/// signs in to the workspace the person picks there. Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> LinearResult<()> {
    let callback = oauth::listen().await?;
    let state = oauth::new_state()?;
    let verifier = oauth::new_verifier()?;
    sink.send(json!({ "url": oauth::authorize_url(&state, &verifier) }))
        .map_err(|_| LinearError::Auth("the sign-in was closed".into()))?;
    let code = tokio::time::timeout(BROWSER_WAIT, oauth::code(&callback, &state))
        .await
        .map_err(|_| LinearError::Auth("nobody finished signing in".into()))??;
    drop(callback);
    let tokens = oauth::exchange(&code, &verifier).await?;
    let viewer =
        client::query_with(&oauth::bearer(&tokens.access_token), VIEWER, json!({})).await?;
    let workspace = Workspace {
        sign_in: SignIn::Browser,
        ..workspace_of(&viewer)?
    };
    let id = workspace.id.clone();
    let refresh_token = tokens.refresh_token.clone();
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        config::keychain_write(&workspace, &refresh_token)?;
        let mut config = config::load(&data_dir);
        config.upsert(workspace);
        config::save(&data_dir, &config)
    }))
    .await?;
    oauth::remember(&id, &tokens).await;
    client::forget(&id);
    Ok(())
}

#[derive(Deserialize)]
pub struct WorkspaceRef {
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Signs out the workspace named, or the default one.
pub async fn sign_out(data_dir: &Path, input: WorkspaceRef) -> LinearResult<()> {
    let data_dir = data_dir.to_path_buf();
    let removed = config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let Some(id) = config
            .workspace(input.workspace.as_deref())
            .map(|workspace| workspace.id.clone())
        else {
            return Ok(None);
        };
        let Some(workspace) = config.remove(&id) else {
            return Ok(None);
        };
        let refresh_token = match workspace.sign_in {
            SignIn::Browser => config::keychain_read(&workspace)?,
            SignIn::Key => None,
        };
        config::keychain_delete(&workspace)?;
        config::save(&data_dir, &config)?;
        Ok(Some((workspace, refresh_token)))
    }))
    .await?;
    if let Some((workspace, refresh_token)) = removed {
        client::forget(&workspace.id);
        oauth::forget(&workspace.id).await;
        if let Some(refresh_token) = refresh_token {
            oauth::revoke(&refresh_token).await;
        }
    }
    Ok(())
}

/// Makes the workspace named the one calls use when they name none.
pub async fn set_default(data_dir: &Path, input: WorkspaceRef) -> LinearResult<()> {
    let data_dir = data_dir.to_path_buf();
    config::blocking(Box::new(move || {
        let mut config = config::load(&data_dir);
        let id = config
            .workspace(input.workspace.as_deref())
            .map(|workspace| workspace.id.clone())
            .ok_or_else(|| LinearError::NotFound("no such Linear workspace is signed in".into()))?;
        config.default = Some(id);
        config::save(&data_dir, &config)
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_belongs_to_the_viewer_in_their_workspace() {
        let data = json!({ "viewer": { "id": "u1", "name": "Ankit Goyal", "displayName": "ankit", "organization": { "id": "o1", "name": "Acme", "urlKey": "acme" } } });
        assert_eq!(
            workspace_of(&data).expect("reads"),
            Workspace {
                id: "o1".into(),
                name: "Acme".into(),
                url_key: "acme".into(),
                user_id: "u1".into(),
                user: "ankit".into(),
                sign_in: SignIn::Key,
            }
        );
        assert!(workspace_of(&json!({ "viewer": {} })).is_err());
    }
}
