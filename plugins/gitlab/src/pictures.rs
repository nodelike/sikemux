// A picture for a merge request's description goes to the project's uploads, as GitLab's own
// editor puts one there.

use std::path::Path;

use serde::Deserialize;
use sikemux_plugin_api::codehost::asks::PictureFile;
use sikemux_plugin_api::codehost::Attached;

use crate::client;
use crate::error::{GitlabError, GitlabResult};
use crate::repo::ApiPath;

#[derive(Deserialize)]
struct Uploaded {
    url: String,
}

#[derive(Deserialize)]
struct Project {
    web_url: String,
}

pub async fn attach(data_dir: &Path, input: PictureFile) -> GitlabResult<Attached> {
    let picture = input.read().await.map_err(GitlabError::BadArg)?;
    let (kind, form) = picture.form("file");
    let uploaded: Uploaded = client::API
        .upload(data_dir, &input.repo.path("/uploads")?, &kind, form)
        .await?;
    let project: Project = client::API
        .get(data_dir, &input.repo.path("")?, &[])
        .await?;
    Ok(Attached {
        url: format!("{}{}", project.web_url.trim_end_matches('/'), uploaded.url),
    })
}
