// Releases, their notes, and the files hung off them.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_plugin_api::{PluginResult, StreamSink};

use crate::artifacts;
use crate::client;
use crate::common::{login_of, ActorRow, RELEASE_PAGES};
use crate::error::GithubResult;
use crate::workflows::RepoRef;

#[derive(Deserialize)]
struct AssetRow {
    id: u64,
    name: String,
    size: u64,
    download_count: Option<u64>,
}

#[derive(Deserialize)]
struct ReleaseRow {
    id: u64,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    created_at: Option<String>,
    published_at: Option<String>,
    author: Option<ActorRow>,
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetRow>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: u64,
    pub name: String,
    pub size_bytes: u64,
    pub downloads: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub id: u64,
    pub tag: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    pub prerelease: bool,
    /// A draft has never been published, so it has only a creation time.
    pub published_at: Option<String>,
    pub author: Option<String>,
    pub assets: Vec<Asset>,
    pub url: String,
}

impl From<ReleaseRow> for Release {
    fn from(row: ReleaseRow) -> Self {
        Self {
            id: row.id,
            name: row
                .name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| row.tag_name.clone()),
            tag: row.tag_name,
            body: row.body.unwrap_or_default(),
            draft: row.draft,
            prerelease: row.prerelease,
            published_at: row.published_at.or(row.created_at),
            author: login_of(&row.author),
            assets: row
                .assets
                .into_iter()
                .map(|asset| Asset {
                    id: asset.id,
                    name: asset.name,
                    size_bytes: asset.size,
                    downloads: asset.download_count.unwrap_or(0),
                })
                .collect(),
            url: row.html_url,
        }
    }
}

pub async fn list(data_dir: &Path, repo: RepoRef) -> GithubResult<Vec<Release>> {
    let rows: Vec<ReleaseRow> = client::get_all(
        data_dir,
        &repo.path("/releases")?,
        &[],
        RELEASE_PAGES,
        |rows| rows,
    )
    .await?;
    Ok(rows.into_iter().map(Release::from).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAsset {
    #[serde(flatten)]
    pub repo: RepoRef,
    pub asset_id: u64,
    /// What to call the file, which is not the repository's `name`.
    pub file_name: String,
}

/// A release asset is whatever was uploaded, so it keeps its own name rather
/// than becoming a zip the way an artifact does.
pub async fn download(
    data_dir: &Path,
    input: DownloadAsset,
    sink: &StreamSink,
) -> PluginResult<()> {
    let path = input
        .repo
        .path(&format!("/releases/assets/{}", input.asset_id))?;
    let name = artifacts::file_name(&input.file_name, None);
    artifacts::save(data_dir, &path, "application/octet-stream", &name, sink).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(value: serde_json::Value) -> ReleaseRow {
        serde_json::from_value(value).expect("parses")
    }

    #[test]
    fn a_release_with_no_name_is_known_by_its_tag() {
        let unnamed = Release::from(row(json!({
            "id": 1, "tag_name": "v0.4.2", "name": null, "draft": false, "prerelease": false,
            "html_url": "https://github.com/a/b/releases/tag/v0.4.2",
        })));
        assert_eq!(unnamed.name, "v0.4.2");

        let empty = Release::from(row(json!({
            "id": 1, "tag_name": "v0.4.2", "name": "", "draft": false, "prerelease": false,
            "html_url": "https://github.com/a/b/releases/tag/v0.4.2",
        })));
        assert_eq!(empty.name, "v0.4.2");
    }

    #[test]
    fn a_draft_falls_back_to_when_it_was_made() {
        let draft = Release::from(row(json!({
            "id": 1, "tag_name": "v0.5.0", "draft": true, "prerelease": false,
            "created_at": "2026-01-01T00:00:00Z", "published_at": null,
            "html_url": "https://github.com/a/b/releases",
        })));
        assert!(draft.draft);
        assert_eq!(draft.published_at.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn reads_the_files_hung_off_a_release() {
        let release = Release::from(row(json!({
            "id": 1, "tag_name": "v0.4.2", "draft": false, "prerelease": true,
            "html_url": "https://github.com/a/b/releases/tag/v0.4.2",
            "assets": [{ "id": 9, "name": "Sikemux_aarch64.dmg", "size": 10_485_760, "download_count": 42 }],
        })));
        assert!(release.prerelease);
        let asset = release.assets.first().expect("one asset");
        assert_eq!(asset.name, "Sikemux_aarch64.dmg");
        assert_eq!(asset.downloads, 42);
    }
}
