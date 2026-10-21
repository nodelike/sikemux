// The projects an account can see.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, text};
use crate::error::GcpResult;

const PROJECTS: &str = "https://cloudresourcemanager.googleapis.com/v1/projects";
const MAX_PROJECTS: usize = 500;

pub fn project(value: &Value) -> Value {
    json!({
        "id": at(value, "/projectId"),
        "name": at(value, "/name"),
        "number": at(value, "/projectNumber"),
        "createdAt": at(value, "/createTime"),
        "labels": at(value, "/labels"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
    #[serde(default)]
    pub query: Option<String>,
}

/// The account's active projects, by id and name, filtered by words when there are some.
pub async fn projects(input: Listing) -> GcpResult<Vec<Value>> {
    let listed = client::list(
        &input.target,
        PROJECTS,
        &[("filter", "lifecycleState:ACTIVE".into())],
        "projects",
        MAX_PROJECTS,
    )
    .await?;
    let words = input.query.unwrap_or_default().trim().to_lowercase();
    let mut projects: Vec<Value> = listed
        .iter()
        .filter(|raw| {
            words.is_empty()
                || text(raw, "/projectId").to_lowercase().contains(&words)
                || text(raw, "/name").to_lowercase().contains(&words)
        })
        .map(project)
        .collect();
    projects.sort_by_key(|project| text(project, "/id"));
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_reads_by_id_name_and_number() {
        let read = project(
            &json!({ "projectId": "acme-prod", "name": "Acme Prod", "projectNumber": "1234", "lifecycleState": "ACTIVE", "createTime": "2024-01-01T00:00:00Z" }),
        );
        assert_eq!(read["id"], "acme-prod");
        assert_eq!(read["name"], "Acme Prod");
        assert_eq!(read["number"], "1234");
    }
}
