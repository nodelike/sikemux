// Signing workspaces in and out, through the browser or with a personal API key,
// and saying who the sign-in belongs to.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use sikemux_plugin_api::oauth::Tokens;
use sikemux_plugin_api::store::{self, Accounts};
use sikemux_plugin_api::{SignInStatus, StreamSink};

use crate::client::{self, at, Session};
use crate::config::{self, LinearConfig, SignIn, Workspace};
use crate::error::{LinearError, LinearResult};
use crate::oauth;

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

#[derive(Serialize, Default)]
pub struct Signed {
    pub workspaces: Vec<Listed>,
}

pub type Status = SignInStatus<Signed>;

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
            url_key: workspace.url_key.clone(),
            user: workspace.user.clone(),
            is_default: config.default.as_deref() == Some(workspace.id.as_str()),
        })
        .collect();
    let check = match Session::of(data_dir, None).await {
        Ok(session) => session.query(VIEWER, json!({})).await,
        Err(error) => Err(error),
    };
    Status::checked(!config.workspaces.is_empty(), Signed { workspaces }, &check)
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
    store::update(data_dir, move |config: &mut LinearConfig| {
        config::KEYCHAIN.write(&workspace.id, &key)?;
        config.upsert(workspace);
        Ok(())
    })
    .await?;
    client::forget(&id);
    Ok(id)
}

/// Opens Linear's sign-in in the browser, through the URL sent down the stream, and
/// signs in to the workspace the person picks there. Stopping the stream stops the wait.
pub async fn sign_in_with_browser(data_dir: &Path, sink: &StreamSink) -> LinearResult<()> {
    let tokens: Tokens = oauth::APP.sign_in(&client::HTTP, sink).await?;
    let viewer =
        client::query_with(&oauth::bearer(&tokens.access_token), VIEWER, json!({})).await?;
    let workspace = Workspace {
        sign_in: SignIn::Browser,
        ..workspace_of(&viewer)?
    };
    let id = workspace.id.clone();
    let refresh_token = tokens.refresh_token.clone();
    store::update(data_dir, move |config: &mut LinearConfig| {
        config::KEYCHAIN.write(&workspace.id, &refresh_token)?;
        config.upsert(workspace);
        Ok(())
    })
    .await?;
    oauth::ACCESS.remember(&id, &tokens).await;
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
    let removed = store::update(data_dir, move |config: &mut LinearConfig| {
        let Some(id) = config
            .account(input.workspace.as_deref())
            .map(|workspace| workspace.id.clone())
        else {
            return Ok(None);
        };
        let Some(workspace) = config.remove(&id) else {
            return Ok(None);
        };
        let refresh_token = match workspace.sign_in {
            SignIn::Browser => config::KEYCHAIN.read(&workspace.id)?,
            SignIn::Key => None,
        };
        config::KEYCHAIN.delete(&workspace.id)?;
        Ok(Some((workspace, refresh_token)))
    })
    .await?;
    if let Some((workspace, refresh_token)) = removed {
        client::forget(&workspace.id);
        oauth::ACCESS.forget(Some(&workspace.id)).await;
        if let Some(refresh_token) = refresh_token {
            oauth::revoke(&refresh_token).await;
        }
    }
    Ok(())
}

/// Makes the workspace named the one calls use when they name none.
pub async fn set_default(data_dir: &Path, input: WorkspaceRef) -> LinearResult<()> {
    store::update(data_dir, move |config: &mut LinearConfig| {
        let id = config
            .account(input.workspace.as_deref())
            .map(|workspace| workspace.id.clone())
            .ok_or_else(|| LinearError::NotFound("no such Linear workspace is signed in".into()))?;
        config.set_default(&id)
    })
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
