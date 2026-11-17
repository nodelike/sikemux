// Bitbucket keeps no pictures for a description on its own, so a picture goes to the
// repository's downloads, which its pages show to whoever can read the repository.

use std::path::Path;

use serde_json::Value;
use sikemux_plugin_api::codehost::asks::PictureFile;
use sikemux_plugin_api::codehost::Attached;

use crate::client;
use crate::error::{BitbucketError, BitbucketResult};
use crate::repo::Place;

pub async fn attach(data_dir: &Path, input: PictureFile) -> BitbucketResult<Attached> {
    let picture = input.read().await.map_err(BitbucketError::BadArg)?;
    let (kind, form) = picture.form("files");
    let _: Value = client::API
        .upload(data_dir, &input.repo.path("/downloads")?, &kind, form)
        .await?;
    Ok(Attached {
        url: input.repo.web(&format!("/downloads/{}", picture.name)),
    })
}
