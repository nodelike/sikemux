//! A project opened on another host of the account (ADR 0019). That host opens
//! a checkout it has of the same `origin`, or clones the project from its
//! remote with its own sign-in, or takes it from this host as a git bundle when
//! it cannot, and adds it to its projects. Only committed work goes over.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::connection::blocking;
use super::super::workspace::{self, AddProjectParams};
use super::super::{Core, CoreResult};
use super::calls::{self as asks, params_of, HostAsk};
use super::code::{self, Code, Opened};
use super::link::{Answer, Received};
use super::{ask, find, StoredLink};

/// Long enough to clone a large repository, or to take one from a bundle.
const OPEN_WAIT: Duration = Duration::from_secs(10 * 60);

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOnParams {
    pub path: PathBuf,
    pub host: String,
    /// What is not committed goes too, as a commit on top.
    #[serde(default)]
    pub uncommitted: bool,
}

/// Opens the project at `path` on the linked host `host`, and answers with
/// that host's project for it.
pub(crate) async fn open_on(core: &Arc<Core>, params: OpenOnParams) -> CoreResult<Value> {
    let link = find(core, &params.host)?;
    open_there(core, &link, params.path, params.uncommitted, OpenAs::Copy).await
}

/// Where on the other host a project opened there goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAs {
    /// A new checkout of its own, with what is not committed committed on top.
    Copy,
    /// The checkout there of the same repository when it stands where this one does, with
    /// what is not committed laid over it; else nothing changes and the answer says why.
    Checkout,
    /// A worktree at this commit of the checkout there, else a new checkout.
    Worktree,
}

/// Opens the project at `path` on the linked host `link`, with what is not committed going
/// too when `uncommitted`, and answers with that host's project for it, the folder the work
/// is in there and how the uncommitted changes went, or why the checkout there differs.
pub(crate) async fn open_there(
    core: &Arc<Core>,
    link: &StoredLink,
    path: PathBuf,
    uncommitted: bool,
    how: OpenAs,
) -> CoreResult<Value> {
    let packed = blocking(move || code::package(&path).map_err(Into::into)).await?;
    let patch = packed.code.patch.clone().filter(|_| uncommitted);
    let opening = OpenParams {
        code: Code {
            patch,
            ..packed.code.clone()
        },
        copy: how == OpenAs::Copy,
        into: None,
        abandon: None,
        worktree: how == OpenAs::Worktree,
    };
    let asked = |opening: &OpenParams, body: Option<&Path>| {
        let params = params_of::<asks::OpenProject>(opening);
        let body = body.map(Path::to_path_buf);
        async move {
            let method = asks::OpenProject::METHOD;
            ask(core, &link.id, method, params, body.as_deref(), OPEN_WAIT).await
        }
    };
    let answer = asked(&opening, None)
        .await
        .map_err(|error| error.to_string())?;
    if answer["needs"] != "bundle" {
        return Ok(answer);
    }
    let tips: Vec<String> = answer["tips"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let into = answer["into"].as_str().map(PathBuf::from);
    let (repo, commit) = (packed.repo, packed.code.commit);
    let bundled = blocking(move || code::bundle(&repo, &commit, &tips).map_err(Into::into)).await;
    let answer = match bundled {
        Ok(bundle) => {
            let second = OpenParams {
                into: into.clone(),
                ..opening.clone()
            };
            let sent = asked(&second, Some(bundle.path())).await;
            sent.map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    };
    let failed = match answer {
        Ok(answer) if answer["needs"] != "bundle" => return Ok(answer),
        Ok(_) => format!(
            "{} could not take the project from this host's bundle",
            link.name
        ),
        Err(error) => error,
    };
    if let Some(into) = into {
        let leave = OpenParams {
            abandon: Some(into),
            ..opening
        };
        let _ = asked(&leave, None).await;
    }
    Err(failed.into())
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct OpenParams {
    code: Code,
    /// A new copy (ADR 0023): a checkout of its own, never one already there, with what is
    /// not committed committed on top.
    #[serde(default)]
    copy: bool,
    /// The folder the first round claimed, which the bundle of the second goes into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    into: Option<PathBuf>,
    /// A folder the first round claimed for an opening that will not finish, to remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    abandon: Option<PathBuf>,
    /// The work goes into a worktree at the commit sent, when the checkout there stands
    /// elsewhere.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    worktree: bool,
}

pub(super) async fn open_here(
    core: &Arc<Core>,
    params: OpenParams,
    body: Option<Received>,
) -> Answer {
    let OpenParams {
        code,
        copy,
        into,
        abandon,
        worktree,
    } = params;
    if let Some(dir) = abandon {
        blocking(move || {
            code::abandon(&dir);
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())?;
        return Ok(Value::Null);
    }
    let opened = {
        let (core, code) = (core.clone(), code.clone());
        blocking(move || {
            let bundle = body.as_ref().map(Received::path);
            let into = into.as_deref();
            code::open_project(&core, &code, bundle, copy, into, worktree).map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    match opened {
        Opened::Needs { into, tips } => {
            Ok(json!({ "needs": "bundle", "tips": tips, "into": into }))
        }
        Opened::Differs(why) => Ok(json!({ "differs": why })),
        Opened::Ready { repo, work } => {
            let laid = match &code.patch {
                Some(patch) => {
                    let (path, patch) = (work.clone(), patch.clone());
                    let laid = blocking(move || {
                        match copy {
                            true => code::commit_over(&path, &patch),
                            false => code::lay_over(&path, &patch),
                        }
                        .map_err(Into::into)
                    });
                    Some(
                        laid.await
                            .map_or_else(|error| error.to_string(), |()| "applied".into()),
                    )
                }
                None => None,
            };
            let project = workspace::add_project(core, AddProjectParams::named(repo, code.project))
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "path": project.path,
                "project": project.id,
                "work": work,
                "uncommitted": laid,
            }))
        }
    }
}
