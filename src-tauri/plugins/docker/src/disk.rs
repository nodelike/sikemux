//! How much disk the engine uses, and how much of it could be cleared away, as `docker system df` shows.

use serde::Serialize;

use crate::engine::Engine;
use crate::error::DockerResult;

#[derive(Debug, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Disk {
    pub images: Usage,
    pub containers: Usage,
    pub volumes: Usage,
    pub build_cache: Usage,
}

#[derive(Debug, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub count: usize,
    pub active: usize,
    pub size: u64,
    /// What removing the unused ones would free.
    pub reclaimable: u64,
}

mod raw {
    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct SystemDf {
        pub layers_size: i64,
        pub images: Option<Vec<Image>>,
        pub containers: Option<Vec<Container>>,
        pub volumes: Option<Vec<Volume>>,
        pub build_cache: Option<Vec<Cache>>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct Image {
        pub size: i64,
        pub shared_size: i64,
        pub containers: i64,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct Container {
        pub size_rw: i64,
        pub state: String,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct Volume {
        pub usage_data: Option<UsageData>,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct UsageData {
        pub size: i64,
        pub ref_count: i64,
    }

    #[derive(Deserialize, Default)]
    #[serde(rename_all = "PascalCase", default)]
    pub struct Cache {
        pub size: i64,
        pub in_use: bool,
        pub shared: bool,
    }
}

/// The engine writes -1 for a size it hasn't measured.
fn bytes(size: i64) -> u64 {
    u64::try_from(size).unwrap_or(0)
}

fn disk_of(df: raw::SystemDf) -> Disk {
    let images = df.images.unwrap_or_default();
    let unused_images: u64 = images
        .iter()
        .filter(|image| image.containers == 0)
        .map(|image| bytes(image.size - image.shared_size.max(0)))
        .sum();
    let containers = df.containers.unwrap_or_default();
    let volumes = df.volumes.unwrap_or_default();
    let measured = |volume: &raw::Volume| {
        volume
            .usage_data
            .as_ref()
            .map_or(0, |usage| bytes(usage.size))
    };
    let unused = |volume: &&raw::Volume| {
        volume
            .usage_data
            .as_ref()
            .is_some_and(|usage| usage.ref_count == 0)
    };
    let cache = df.build_cache.unwrap_or_default();
    Disk {
        images: Usage {
            count: images.len(),
            active: images.iter().filter(|image| image.containers > 0).count(),
            size: bytes(df.layers_size),
            reclaimable: unused_images,
        },
        containers: Usage {
            count: containers.len(),
            active: containers
                .iter()
                .filter(|container| container.state == "running")
                .count(),
            size: containers
                .iter()
                .map(|container| bytes(container.size_rw))
                .sum(),
            reclaimable: containers
                .iter()
                .filter(|container| container.state != "running")
                .map(|container| bytes(container.size_rw))
                .sum(),
        },
        volumes: Usage {
            count: volumes.len(),
            active: volumes.len() - volumes.iter().filter(unused).count(),
            size: volumes.iter().map(measured).sum(),
            reclaimable: volumes.iter().filter(unused).map(measured).sum(),
        },
        build_cache: Usage {
            count: cache.len(),
            active: cache.iter().filter(|entry| entry.in_use).count(),
            size: cache
                .iter()
                .filter(|entry| !entry.shared)
                .map(|entry| bytes(entry.size))
                .sum(),
            reclaimable: cache
                .iter()
                .filter(|entry| !entry.in_use && !entry.shared)
                .map(|entry| bytes(entry.size))
                .sum(),
        },
    }
}

pub async fn read(engine: &Engine) -> DockerResult<Disk> {
    let df: raw::SystemDf = engine.get("/system/df").await?;
    Ok(disk_of(df))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_up_as_docker_system_df_does() {
        let df: raw::SystemDf = serde_json::from_str(
            r#"{"LayersSize":1000,
                "Images":[{"Size":600,"SharedSize":100,"Containers":0},{"Size":400,"SharedSize":-1,"Containers":2}],
                "Containers":[{"SizeRw":10,"State":"running"},{"SizeRw":30,"State":"exited"}],
                "Volumes":[{"UsageData":{"Size":50,"RefCount":1}},{"UsageData":{"Size":70,"RefCount":0}},{"UsageData":{"Size":-1,"RefCount":-1}}],
                "BuildCache":[{"Size":5,"InUse":true},{"Size":7,"InUse":false},{"Size":9,"Shared":true}]}"#,
        )
        .unwrap();
        let disk = disk_of(df);
        assert_eq!(
            disk.images,
            Usage {
                count: 2,
                active: 1,
                size: 1000,
                reclaimable: 500
            }
        );
        assert_eq!(
            disk.containers,
            Usage {
                count: 2,
                active: 1,
                size: 40,
                reclaimable: 30
            }
        );
        assert_eq!(
            disk.volumes,
            Usage {
                count: 3,
                active: 2,
                size: 120,
                reclaimable: 70
            }
        );
        assert_eq!(
            disk.build_cache,
            Usage {
                count: 3,
                active: 1,
                size: 12,
                reclaimable: 7
            }
        );
    }
}
