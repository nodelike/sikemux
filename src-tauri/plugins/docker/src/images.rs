//! Images: what is pulled or built here, which containers use each, and clearing them away.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::containers::short_id;
use crate::engine::{query_value, segment, Engine};
use crate::error::{DockerError, DockerResult};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub id: String,
    /// Such as `postgres:16`. An image no tag points to has none.
    pub tags: Vec<String>,
    pub created: i64,
    pub size: u64,
    /// Containers made from it, running or not.
    pub containers: u32,
    /// No tag points to it, as happens to the old image when a tag is built again.
    pub dangling: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Summary {
    id: String,
    repo_tags: Option<Vec<String>>,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct ContainerImage {
    #[serde(rename = "ImageID", default)]
    image_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ListParams {
    /// Words in a tag.
    pub query: Option<String>,
}

fn without_prefix(id: &str) -> &str {
    id.strip_prefix("sha256:").unwrap_or(id)
}

pub async fn list(engine: &Engine, params: ListParams) -> DockerResult<Vec<Image>> {
    let summaries: Vec<Summary> = engine.get("/images/json").await?;
    let users: Vec<ContainerImage> = engine.get("/containers/json?all=true").await?;
    let mut uses: BTreeMap<&str, u32> = BTreeMap::new();
    for user in &users {
        *uses.entry(without_prefix(&user.image_id)).or_default() += 1;
    }
    let words = params
        .query
        .as_deref()
        .map(str::to_lowercase)
        .filter(|words| !words.trim().is_empty());
    let mut images: Vec<Image> = summaries
        .into_iter()
        .map(|summary| {
            let tags: Vec<String> = summary
                .repo_tags
                .unwrap_or_default()
                .into_iter()
                .filter(|tag| tag != "<none>:<none>")
                .collect();
            let id = without_prefix(&summary.id);
            Image {
                containers: uses.get(id).copied().unwrap_or(0),
                id: short_id(id),
                dangling: tags.is_empty(),
                tags,
                created: summary.created,
                size: summary.size,
            }
        })
        .filter(|image| {
            words.as_deref().is_none_or(|words| {
                image
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(words))
            })
        })
        .collect();
    images.sort_by_key(|image| std::cmp::Reverse(image.created));
    Ok(images)
}

#[derive(Deserialize)]
pub struct RemoveParams {
    /// A tag or id.
    pub id: String,
    /// Remove it even when a stopped container was made from it.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub untagged: Vec<String>,
    pub deleted: usize,
    pub space_reclaimed: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Deletion {
    untagged: Option<String>,
    deleted: Option<String>,
}

pub async fn remove(engine: &Engine, params: RemoveParams) -> DockerResult<Removed> {
    let id = segment(&params.id)?;
    let deletions: Vec<Deletion> = engine
        .delete_json(&format!("/images/{id}?force={}", params.force))
        .await
        .map_err(|error| match error {
            DockerError::Conflict(message) if !params.force => DockerError::Conflict(format!(
                "{message}. Remove the containers made from it first, or remove it with force"
            )),
            other => other,
        })?;
    Ok(Removed {
        untagged: deletions
            .iter()
            .filter_map(|each| each.untagged.clone())
            .collect(),
        deleted: deletions
            .iter()
            .filter(|each| each.deleted.is_some())
            .count(),
        space_reclaimed: 0,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Pruned {
    images_deleted: Option<Vec<Deletion>>,
    #[serde(default)]
    space_reclaimed: u64,
}

/// Removes only the images no tag points to and no container uses.
pub async fn prune(engine: &Engine) -> DockerResult<Removed> {
    let filters = query_value(r#"{"dangling":["true"]}"#);
    let pruned: Pruned = engine
        .post_json(&format!("/images/prune?filters={filters}"), None)
        .await?;
    let deletions = pruned.images_deleted.unwrap_or_default();
    Ok(Removed {
        untagged: deletions
            .iter()
            .filter_map(|each| each.untagged.clone())
            .collect(),
        deleted: deletions
            .iter()
            .filter(|each| each.deleted.is_some())
            .count(),
        space_reclaimed: pruned.space_reclaimed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fake_engine;

    #[tokio::test]
    async fn images_list_newest_first_with_the_containers_made_from_them() {
        let fake = fake_engine(vec![
            (
                200,
                "application/json",
                r#"[{"Id":"sha256:aaaaaaaaaaaaffff","RepoTags":["postgres:16"],"Created":100,"Size":400},
                    {"Id":"sha256:bbbbbbbbbbbbffff","RepoTags":["<none>:<none>"],"Created":300,"Size":90},
                    {"Id":"sha256:ccccccccccccffff","RepoTags":null,"Created":200,"Size":50}]"#
                    .to_owned(),
            ),
            (
                200,
                "application/json",
                r#"[{"ImageID":"sha256:aaaaaaaaaaaaffff"},{"ImageID":"sha256:aaaaaaaaaaaaffff"}]"#.to_owned(),
            ),
        ]);
        let images = list(&fake.engine, ListParams::default()).await.unwrap();
        let ids: Vec<&str> = images.iter().map(|image| image.id.as_str()).collect();
        assert_eq!(ids, ["bbbbbbbbbbbb", "cccccccccccc", "aaaaaaaaaaaa"]);
        assert!(images[0].dangling && images[1].dangling && !images[2].dangling);
        assert_eq!(images[2].containers, 2);
    }

    #[tokio::test]
    async fn removing_an_image_in_use_says_how_to() {
        let fake = fake_engine(vec![(
            409,
            "application/json",
            r#"{"message":"conflict: unable to remove repository reference \"postgres:16\" (must force)"}"#.to_owned(),
        )]);
        let error = remove(
            &fake.engine,
            RemoveParams {
                id: "postgres:16".to_owned(),
                force: false,
            },
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().ends_with("or remove it with force"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn pruning_asks_only_for_dangling_images_and_says_what_it_freed() {
        let mut fake = fake_engine(vec![(
            200,
            "application/json",
            r#"{"ImagesDeleted":[{"Deleted":"sha256:bbbb"},{"Untagged":"old@sha256:1"}],"SpaceReclaimed":90}"#.to_owned(),
        )]);
        let removed = prune(&fake.engine).await.unwrap();
        assert_eq!(
            removed,
            Removed {
                untagged: vec!["old@sha256:1".to_owned()],
                deleted: 1,
                space_reclaimed: 90
            }
        );
        let request = fake.seen.recv().await.unwrap();
        assert!(
            request.starts_with(
                "POST /images/prune?filters=%7B%22dangling%22%3A%5B%22true%22%5D%7D HTTP/1.1"
            ),
            "{request}"
        );
    }
}
