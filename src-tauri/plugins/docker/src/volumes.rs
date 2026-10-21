//! Volumes: how big each is, which containers use it, and clearing away the unused ones.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{segment, Engine};
use crate::error::{DockerError, DockerResult};

const ANONYMOUS: &str = "com.docker.volume.anonymous";
const COMPOSE_PROJECT: &str = "com.docker.compose.project";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub created: Option<String>,
    /// Bytes on disk, when the engine has measured it.
    pub size: Option<u64>,
    /// The containers that mount it, running or not.
    pub used_by: Vec<String>,
    /// Made by a container that didn't name it, so nothing will look for it again once it is unused.
    pub anonymous: bool,
    pub compose_project: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Usage {
    #[serde(default)]
    volumes: Option<Vec<RawVolume>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawVolume {
    name: String,
    #[serde(default)]
    driver: String,
    created_at: Option<String>,
    labels: Option<BTreeMap<String, String>>,
    usage_data: Option<UsageData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UsageData {
    size: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Mounter {
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    mounts: Vec<MountOf>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MountOf {
    #[serde(rename = "Type", default)]
    kind: String,
    name: Option<String>,
}

pub async fn list(engine: &Engine) -> DockerResult<Vec<Volume>> {
    let usage: Usage = engine.get("/system/df?type=volume").await?;
    let mounters: Vec<Mounter> = engine.get("/containers/json?all=true").await?;
    let mut used_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for mounter in mounters {
        let Some(name) = mounter.names.first() else {
            continue;
        };
        let name = name.trim_start_matches('/');
        for mount in mounter.mounts.iter().filter(|mount| mount.kind == "volume") {
            if let Some(volume) = &mount.name {
                used_by
                    .entry(volume.clone())
                    .or_default()
                    .push(name.to_owned());
            }
        }
    }
    let mut volumes: Vec<Volume> = usage
        .volumes
        .unwrap_or_default()
        .into_iter()
        .map(|raw| {
            let labels = raw.labels.unwrap_or_default();
            Volume {
                used_by: used_by.remove(&raw.name).unwrap_or_default(),
                anonymous: labels.contains_key(ANONYMOUS),
                compose_project: labels.get(COMPOSE_PROJECT).cloned(),
                size: raw
                    .usage_data
                    .and_then(|usage| u64::try_from(usage.size).ok()),
                created: raw.created_at.filter(|created| !created.is_empty()),
                driver: raw.driver,
                name: raw.name,
            }
        })
        .collect();
    volumes.sort_by(|a, b| {
        (a.anonymous, &a.compose_project, &a.name).cmp(&(b.anonymous, &b.compose_project, &b.name))
    });
    Ok(volumes)
}

#[derive(Deserialize)]
pub struct RemoveParams {
    pub name: String,
}

pub async fn remove(engine: &Engine, params: RemoveParams) -> DockerResult<()> {
    let name = segment(&params.name)?;
    engine
        .delete(&format!("/volumes/{name}"))
        .await
        .map_err(|error| match error {
            DockerError::Conflict(message) => DockerError::Conflict(format!(
                "{message}. Remove the containers that use it first"
            )),
            other => other,
        })
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pruned {
    pub deleted: Vec<String>,
    pub space_reclaimed: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawPruned {
    volumes_deleted: Option<Vec<String>>,
    #[serde(default)]
    space_reclaimed: u64,
}

/// Removes the anonymous volumes no container uses. Named ones are left, since
/// a Compose stack brought up again would look for them.
pub async fn prune(engine: &Engine) -> DockerResult<Pruned> {
    let pruned: RawPruned = engine.post_json("/volumes/prune", None).await?;
    Ok(Pruned {
        deleted: pruned.volumes_deleted.unwrap_or_default(),
        space_reclaimed: pruned.space_reclaimed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::fake_engine;

    #[tokio::test]
    async fn volumes_list_with_their_size_project_and_the_containers_that_mount_them() {
        let fake = fake_engine(vec![
            (
                200,
                "application/json",
                r#"{"Volumes":[
                    {"Name":"5757d2f3","Driver":"local","CreatedAt":"2026-09-20T12:59:00+05:30","Labels":{"com.docker.volume.anonymous":""},"UsageData":{"RefCount":0,"Size":0}},
                    {"Name":"shop_data","Driver":"local","CreatedAt":"2026-08-21T11:24:02+05:30","Labels":{"com.docker.compose.project":"shop"},"UsageData":{"RefCount":1,"Size":4096}},
                    {"Name":"cache","Driver":"local","Labels":null,"UsageData":{"RefCount":-1,"Size":-1}}]}"#
                    .to_owned(),
            ),
            (
                200,
                "application/json",
                r#"[{"Names":["/shop-db-1"],"Mounts":[{"Type":"volume","Name":"shop_data"},{"Type":"bind","Source":"/Users/me"}]}]"#.to_owned(),
            ),
        ]);
        let volumes = list(&fake.engine).await.unwrap();
        let names: Vec<&str> = volumes.iter().map(|volume| volume.name.as_str()).collect();
        assert_eq!(names, ["cache", "shop_data", "5757d2f3"]);
        assert_eq!(volumes[0].size, None);
        assert_eq!(volumes[1].used_by, ["shop-db-1"]);
        assert_eq!(volumes[1].compose_project.as_deref(), Some("shop"));
        assert!(volumes[2].anonymous);
    }

    #[tokio::test]
    async fn a_volume_in_use_says_what_to_remove_first() {
        let fake = fake_engine(vec![(
            409,
            "application/json",
            r#"{"message":"remove shop_data: volume is in use - [aaaa]"}"#.to_owned(),
        )]);
        let error = remove(
            &fake.engine,
            RemoveParams {
                name: "shop_data".to_owned(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("Remove the containers that use it first"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn pruning_says_which_volumes_went_and_what_it_freed() {
        let fake = fake_engine(vec![(
            200,
            "application/json",
            r#"{"VolumesDeleted":["5757d2f3"],"SpaceReclaimed":1024}"#.to_owned(),
        )]);
        assert_eq!(
            prune(&fake.engine).await.unwrap(),
            Pruned {
                deleted: vec!["5757d2f3".to_owned()],
                space_reclaimed: 1024
            }
        );
    }
}
