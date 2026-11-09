// GitHub takes pictures for a description only from its web page, so a picture is kept on a
// branch of the repository that holds nothing else, and the description shows it from there.

use std::path::Path;

use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use sikemux_plugin_api::codehost::asks::{PictureFile, RepoRef};
use sikemux_plugin_api::codehost::pictures::Picture;
use sikemux_plugin_api::codehost::Attached;
use sikemux_plugin_api::http::Read;

use crate::client;
use crate::error::{GithubError, GithubResult};
use crate::repo::RepoPath;

const BRANCH: &str = "sikemux-evidence";
const FOLDER: &str = "pull-requests";

#[derive(Deserialize)]
struct Content {
    html_url: String,
}

#[derive(Deserialize)]
struct Written {
    content: Content,
}

fn shown(content: Content) -> Attached {
    Attached {
        url: format!("{}?raw=true", content.html_url),
    }
}

pub async fn attach(data_dir: &Path, input: PictureFile) -> GithubResult<Attached> {
    let picture = input.read().await.map_err(GithubError::BadArg)?;
    let file = format!("{FOLDER}/{}", picture.name);
    let path = input.repo.path(&format!("/contents/{file}"))?;
    let body = json!({
        "message": format!("Keep {} for a pull request's description", picture.name),
        "content": picture.base64(),
        "branch": BRANCH,
    });
    let answer = client::API
        .send(data_dir, Method::PUT, &path, &[], Some(&body), Read::ALL)
        .await?;
    match answer.status {
        status if status.is_success() => Ok(shown(answer.parsed::<Written, _>()?.content)),
        StatusCode::UNPROCESSABLE_ENTITY => kept(data_dir, &path).await,
        StatusCode::NOT_FOUND => {
            start_branch(data_dir, &input.repo, &file, &picture).await?;
            kept(data_dir, &path).await
        }
        _ => Err(answer.refused()),
    }
}

/// The picture already on the branch, as one of the same name is the same picture.
async fn kept(data_dir: &Path, path: &str) -> GithubResult<Attached> {
    let content: Content = client::API
        .get(data_dir, path, &[("ref", BRANCH.to_owned())])
        .await?;
    Ok(shown(content))
}

/// Makes the branch with the picture as its only file, on a commit with no parent.
async fn start_branch(
    data_dir: &Path,
    repo: &RepoRef,
    file: &str,
    picture: &Picture,
) -> GithubResult<()> {
    let made = |path: &str, body: serde_json::Value| {
        let path = repo.path(path);
        async move {
            let made: serde_json::Value = client::API
                .send_json(data_dir, Method::POST, &path?, &body)
                .await?;
            Ok::<_, GithubError>(made["sha"].as_str().unwrap_or_default().to_owned())
        }
    };
    let blob = made(
        "/git/blobs",
        json!({ "content": picture.base64(), "encoding": "base64" }),
    )
    .await?;
    let tree = made(
        "/git/trees",
        json!({ "tree": [{ "path": file, "mode": "100644", "type": "blob", "sha": blob }] }),
    )
    .await?;
    let commit = made(
        "/git/commits",
        json!({
            "message": "Pictures Sikemux shows in pull request descriptions",
            "tree": tree,
            "parents": [],
        }),
    )
    .await?;
    let reference = format!("refs/heads/{BRANCH}");
    made("/git/refs", json!({ "ref": reference, "sha": commit })).await?;
    Ok(())
}
