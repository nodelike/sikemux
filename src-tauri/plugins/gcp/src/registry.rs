// Artifact Registry: repositories in every region, with a Docker
// repository's images and their tags, and other formats' packages.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::error::{GcpError, GcpResult};
use crate::run::location_of;

const API: &str = "https://artifactregistry.googleapis.com/v1";
const MAX_ITEMS: usize = 500;

pub fn repository(value: &Value) -> Value {
    let name = text(value, "/name");
    json!({
        "name": short(&name),
        "location": location_of(&name),
        "format": at(value, "/format"),
        "mode": at(value, "/mode"),
        "description": at(value, "/description"),
        "sizeBytes": at(value, "/sizeBytes").as_str().and_then(|size| size.parse::<u64>().ok()),
        "updatedAt": at(value, "/updateTime"),
    })
}

/// A Docker image as the pane and agents read it: its digest, tags, size and when it was pushed.
pub fn image(value: &Value) -> Value {
    let name = text(value, "/name");
    let digest = name.rsplit('@').next().unwrap_or_default().to_string();
    json!({
        "digest": digest,
        "image": short(name.split('@').next().unwrap_or_default()).replace("%2F", "/"),
        "uri": at(value, "/uri"),
        "tags": at(value, "/tags").as_array().cloned().unwrap_or_default(),
        "sizeBytes": at(value, "/imageSizeBytes").as_str().and_then(|size| size.parse::<u64>().ok()),
        "uploadedAt": at(value, "/uploadTime"),
        "builtAt": at(value, "/buildTime"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

async fn regions(target: &Target, project: &str) -> GcpResult<Vec<String>> {
    let page = client::get(target, &format!("{API}/projects/{project}/locations"), &[]).await?;
    Ok(at(&page, "/locations")
        .as_array()
        .into_iter()
        .flatten()
        .map(|location| text(location, "/locationId"))
        .filter(|id| !id.is_empty())
        .collect())
}

/// Every repository in the project, asking region by region when Google will not list them all at once.
pub async fn repositories(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let all = client::list(
        &input.target,
        &format!("{API}/projects/{project}/locations/-/repositories"),
        &[("pageSize", "500".into())],
        "repositories",
        MAX_ITEMS,
    )
    .await;
    let raw = match all {
        Ok(found) => found,
        Err(GcpError::Api { status: 400, .. }) | Err(GcpError::NotFound(_)) => {
            let regions = regions(&input.target, &project).await?;
            let lists = futures::future::join_all(regions.iter().map(|region| {
                let target = input.target.clone();
                let url = format!("{API}/projects/{project}/locations/{region}/repositories");
                async move {
                    client::list(
                        &target,
                        &url,
                        &[("pageSize", "500".into())],
                        "repositories",
                        MAX_ITEMS,
                    )
                    .await
                    .unwrap_or_default()
                }
            }))
            .await;
            lists.into_iter().flatten().collect()
        }
        Err(error) => return Err(error),
    };
    let mut read: Vec<Value> = raw.iter().map(repository).collect();
    read.sort_by_key(|each| text(each, "/name"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct RepositoryRef {
    #[serde(flatten)]
    pub target: Target,
    pub location: String,
    pub repository: String,
}

/// A repository's newest images, or its packages when it is not a Docker repository.
pub async fn contents(input: RepositoryRef) -> GcpResult<Value> {
    let project = input.target.project()?;
    let base = format!(
        "{API}/projects/{project}/locations/{}/repositories/{}",
        segment("region", &input.location)?,
        segment("repository", &input.repository)?
    );
    let repository = client::get(&input.target, &base, &[]).await?;
    if text(&repository, "/format") == "DOCKER" {
        let images = client::list(
            &input.target,
            &format!("{base}/dockerImages"),
            &[
                ("pageSize", "200".into()),
                ("orderBy", "update_time desc".into()),
            ],
            "dockerImages",
            200,
        )
        .await?;
        return Ok(
            json!({ "format": "DOCKER", "images": images.iter().map(image).collect::<Vec<_>>() }),
        );
    }
    let packages = client::list(
        &input.target,
        &format!("{base}/packages"),
        &[("pageSize", "200".into())],
        "packages",
        200,
    )
    .await?;
    Ok(json!({
        "format": at(&repository, "/format"),
        "packages": packages.iter().map(|package| json!({ "name": short(&text(package, "/name")).replace("%2F", "/"), "updatedAt": at(package, "/updateTime") })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_and_an_image_read_plainly() {
        let repo = repository(
            &json!({ "name": "projects/acme/locations/us/repositories/web", "format": "DOCKER", "mode": "STANDARD_REPOSITORY", "sizeBytes": "5368709120" }),
        );
        assert_eq!(
            (repo["name"].as_str(), repo["location"].as_str()),
            (Some("web"), Some("us"))
        );
        assert_eq!(repo["sizeBytes"], 5_368_709_120u64);
        let read = image(
            &json!({ "name": "projects/acme/locations/us/repositories/web/dockerImages/storefront@sha256:abc123", "uri": "us-docker.pkg.dev/acme/web/storefront@sha256:abc123", "tags": ["2.4.0", "latest"], "imageSizeBytes": "120000000" }),
        );
        assert_eq!(read["digest"], "sha256:abc123");
        assert_eq!(read["image"], "storefront");
        assert_eq!(read["tags"], json!(["2.4.0", "latest"]));
    }
}
