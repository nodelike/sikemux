// IAM, read only: who holds which role on the project, by member, and the
// project's service accounts.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, text};
use crate::error::GcpResult;

/// The project's bindings turned around: each member with the roles they hold.
pub fn members_of(policy: &Value) -> Vec<Value> {
    let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for binding in at(policy, "/bindings").as_array().into_iter().flatten() {
        let role = text(binding, "/role")
            .trim_start_matches("roles/")
            .to_string();
        let conditional = binding.get("condition").is_some();
        for member in at(binding, "/members")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            members
                .entry(member.to_string())
                .or_default()
                .push(if conditional {
                    format!("{role} (conditional)")
                } else {
                    role.clone()
                });
        }
    }
    members
        .into_iter()
        .map(|(member, mut roles)| {
            roles.sort();
            let (kind, who) = member.split_once(':').unwrap_or(("", member.as_str()));
            json!({ "member": who, "kind": kind, "roles": roles })
        })
        .collect()
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// Who holds which role on the project.
pub async fn members(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let policy = client::post(
        &input.target,
        &format!("https://cloudresourcemanager.googleapis.com/v1/projects/{project}:getIamPolicy"),
        &json!({ "options": { "requestedPolicyVersion": 3 } }),
    )
    .await?;
    Ok(members_of(&policy))
}

/// The project's service accounts.
pub async fn service_accounts(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("https://iam.googleapis.com/v1/projects/{project}/serviceAccounts"),
        &[("pageSize", "100".into())],
        "accounts",
        500,
    )
    .await?;
    let mut read: Vec<Value> = listed
        .iter()
        .map(|account| {
            json!({
                "email": at(account, "/email"),
                "name": at(account, "/displayName"),
                "description": at(account, "/description"),
                "disabled": at(account, "/disabled").as_bool() == Some(true),
            })
        })
        .collect();
    read.sort_by_key(|each| text(each, "/email"));
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_turn_into_each_members_roles() {
        let members = members_of(&json!({ "bindings": [
            { "role": "roles/owner", "members": ["user:edon@acme.dev"] },
            { "role": "roles/run.admin", "members": ["user:edon@acme.dev", "serviceAccount:deployer@acme.iam.gserviceaccount.com"] },
            { "role": "roles/storage.objectViewer", "members": ["group:support@acme.dev"], "condition": { "title": "office hours" } }
        ] }));
        assert_eq!(members.len(), 3);
        let edon = members
            .iter()
            .find(|m| m["member"] == "edon@acme.dev")
            .expect("edon");
        assert_eq!(edon["kind"], "user");
        assert_eq!(edon["roles"], json!(["owner", "run.admin"]));
        let support = members
            .iter()
            .find(|m| m["kind"] == "group")
            .expect("support");
        assert_eq!(
            support["roles"],
            json!(["storage.objectViewer (conditional)"])
        );
    }
}
