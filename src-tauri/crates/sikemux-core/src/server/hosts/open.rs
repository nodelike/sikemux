//! A project opened on another host of the account (ADR 0019). That host opens
//! a checkout it has of the same `origin`, or clones the project from its
//! remote with its own sign-in, or takes it from this host as a git bundle when
//! it cannot, and adds it to its projects. Only committed work goes over.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::super::connection::blocking;
use super::super::workspace::{self, AddProjectParams};
use super::super::{Core, CoreResult};
use super::code::{self, Code, Opened};
use super::link::{Answer, Received};
use super::{ask, find, LinkedHost};

/// Long enough to clone a large repository, or to take one from a bundle.
const OPEN_WAIT: Duration = Duration::from_secs(10 * 60);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenOnParams {
    path: PathBuf,
    host: String,
}

/// Opens the project at `path` on the linked host `host`, and answers with
/// that host's project for it.
pub(crate) async fn open_on(core: &Arc<Core>, params: OpenOnParams) -> CoreResult<Value> {
    let link = find(core, &params.host)?;
    open_there(core, &link, params.path, false).await
}

/// Opens the project at `path` on the linked host `link`, with what is not committed laid
/// over it there when `uncommitted`, and answers with that host's project for it and, then,
/// how the uncommitted changes went.
pub(crate) async fn open_there(
    core: &Arc<Core>,
    link: &LinkedHost,
    path: PathBuf,
    uncommitted: bool,
) -> CoreResult<Value> {
    let packed = blocking(move || code::package(&path).map_err(Into::into)).await?;
    let patch = packed.code.patch.clone().filter(|_| uncommitted);
    let request = json!({ "code": Code { patch, ..packed.code.clone() } });
    let asked = |body| {
        ask(
            core,
            &link.id,
            "openProject",
            request.clone(),
            body,
            OPEN_WAIT,
        )
    };
    let answer = asked(None).await.map_err(|error| error.to_string())?;
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
    let (repo, commit) = (packed.repo, packed.code.commit);
    let bundle = blocking(move || code::bundle(&repo, &commit, &tips).map_err(Into::into)).await?;
    let answer = asked(Some(bundle.path()))
        .await
        .map_err(|error| error.to_string())?;
    if answer["needs"] == "bundle" {
        return Err(format!(
            "{} could not take the project from this host's bundle",
            link.name
        )
        .into());
    }
    Ok(answer)
}

#[derive(Deserialize)]
struct OpenParams {
    code: Code,
}

/// A linked host asks this one to open the person's project here.
pub(super) async fn open_here(core: &Arc<Core>, params: Value, body: Option<Received>) -> Answer {
    let OpenParams { code } =
        serde_json::from_value(params).map_err(|error| format!("unreadable project: {error}"))?;
    let opened = {
        let (core, code) = (core.clone(), code.clone());
        blocking(move || {
            code::open_project(&core, &code, body.as_ref().map(Received::path)).map_err(Into::into)
        })
        .await
        .map_err(|error| error.to_string())?
    };
    match opened {
        Opened::Needs(tips) => Ok(json!({ "needs": "bundle", "tips": tips })),
        Opened::Ready(path) => {
            let laid = match &code.patch {
                Some(patch) => {
                    let (path, patch) = (path.clone(), patch.clone());
                    let laid = blocking(move || code::lay_over(&path, &patch).map_err(Into::into));
                    Some(
                        laid.await
                            .map_or_else(|error| error.to_string(), |()| "applied".into()),
                    )
                }
                None => None,
            };
            let project = workspace::add_project(core, AddProjectParams::named(path, code.project))
                .await
                .map_err(|error| error.to_string())?;
            Ok(json!({ "path": project.path, "project": project.id, "uncommitted": laid }))
        }
    }
}
