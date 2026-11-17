// What a run left behind.

use std::path::Path;

use serde::Deserialize;
use sikemux_plugin_api::codehost::asks::RunRef;
use sikemux_plugin_api::codehost::Artifact;

use crate::client;
use crate::common::LIST_PAGES;
use crate::error::GithubResult;
use crate::runs::run_path;

#[derive(Deserialize)]
struct ArtifactRow {
    id: u64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    created_at: Option<String>,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct ArtifactList {
    artifacts: Vec<ArtifactRow>,
}

pub async fn list(data_dir: &Path, input: RunRef) -> GithubResult<Vec<Artifact>> {
    let path = run_path(&input, "/artifacts")?;
    let artifacts = client::get_all(data_dir, &path, &[], LIST_PAGES, |list: ArtifactList| {
        list.artifacts
    })
    .await?;
    Ok(artifacts
        .into_iter()
        .map(|row| Artifact {
            id: row.id.to_string(),
            name: row.name,
            size_bytes: row.size_in_bytes,
            expired: row.expired,
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
        .collect())
}
