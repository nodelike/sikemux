// What a subscription holds: its resource groups, and every resource in one place
// through Resource Graph, which answers one query instead of one call per service.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::{self, at, group_of, maybe, text};
use crate::error::{AzureError, AzureResult};

const GROUPS_API: &str = "2025-04-01";
const GRAPH_API: &str = "2022-10-01";
const GRAPH_PATH: &str = "/providers/Microsoft.ResourceGraph/resources";
/// The most rows one Resource Graph answer carries, which is its own limit too.
const GRAPH_LIMIT: u32 = 1000;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub name: String,
    pub location: String,
    pub state: String,
    pub tags: Vec<(String, String)>,
}

fn tags_of(value: &Value, path: &str) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = at(value, path)
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
        .collect();
    tags.sort();
    tags
}

pub fn group_of_value(value: &Value) -> Option<Group> {
    let name = maybe(value, "/name")?;
    Some(Group {
        name,
        location: text(value, "/location"),
        state: text(value, "/properties/provisioningState"),
        tags: tags_of(value, "/tags"),
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct InSubscription {
    /// The subscription id; none uses the CLI's current one.
    pub subscription: Option<String>,
}

async fn subscription_of(named: Option<&str>) -> AzureResult<String> {
    if let Some(named) = named.filter(|named| !named.is_empty()) {
        return client::segment("subscription", named);
    }
    let status = crate::auth::status().await;
    status
        .current
        .map(|subscription| subscription.id)
        .ok_or(AzureError::Unconfigured)
}

pub async fn groups(input: InSubscription) -> AzureResult<Vec<Group>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let listed = client::list(
        &format!("/subscriptions/{subscription}/resourcegroups"),
        GROUPS_API,
        &[],
    )
    .await?;
    let mut groups: Vec<Group> = listed.iter().filter_map(group_of_value).collect();
    groups.sort_by_key(|group| group.name.to_lowercase());
    Ok(groups)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub id: String,
    pub name: String,
    /// The full type, such as microsoft.web/sites.
    pub kind: String,
    /// What the pane calls it, such as Web app or Virtual machine.
    pub label: String,
    pub group: String,
    pub location: String,
    /// Running, Stopped, Succeeded and the like, when the service says.
    pub state: Option<String>,
    pub sku: Option<String>,
    pub tags: Vec<(String, String)>,
}

/// The services the pane knows by name, with what people call each.
const KNOWN: [(&str, &str); 24] = [
    ("microsoft.web/sites", "Web app"),
    ("microsoft.web/serverfarms", "App Service plan"),
    ("microsoft.web/staticsites", "Static web app"),
    ("microsoft.app/containerapps", "Container app"),
    (
        "microsoft.app/managedenvironments",
        "Container apps environment",
    ),
    ("microsoft.compute/virtualmachines", "Virtual machine"),
    ("microsoft.compute/virtualmachinescalesets", "Scale set"),
    ("microsoft.compute/disks", "Disk"),
    ("microsoft.containerservice/managedclusters", "AKS cluster"),
    (
        "microsoft.containerregistry/registries",
        "Container registry",
    ),
    ("microsoft.storage/storageaccounts", "Storage account"),
    ("microsoft.sql/servers", "SQL server"),
    ("microsoft.sql/servers/databases", "SQL database"),
    ("microsoft.documentdb/databaseaccounts", "Cosmos DB account"),
    (
        "microsoft.dbforpostgresql/flexibleservers",
        "Postgres server",
    ),
    ("microsoft.dbformysql/flexibleservers", "MySQL server"),
    ("microsoft.cache/redis", "Redis cache"),
    ("microsoft.keyvault/vaults", "Key vault"),
    ("microsoft.servicebus/namespaces", "Service Bus namespace"),
    ("microsoft.eventhub/namespaces", "Event Hubs namespace"),
    ("microsoft.insights/components", "Application Insights"),
    (
        "microsoft.operationalinsights/workspaces",
        "Log Analytics workspace",
    ),
    ("microsoft.network/publicipaddresses", "Public IP"),
    ("microsoft.network/virtualnetworks", "Virtual network"),
];

/// What people call a resource type, or the type's own last part when it isn't known.
pub fn label_of(kind: &str) -> String {
    let lower = kind.to_lowercase();
    if let Some((_, label)) = KNOWN.iter().find(|(known, _)| *known == lower) {
        return (*label).to_string();
    }
    let tail = lower.rsplit('/').next().unwrap_or(&lower);
    let mut words = tail.replace('_', " ");
    if let Some(first) = words.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    words
}

/// The one word a resource's own fields give for how it is doing.
pub fn state_of(properties: &Value, kind: &str) -> Option<String> {
    let lower = kind.to_lowercase();
    if lower == "microsoft.compute/virtualmachines" {
        if let Some(state) = maybe(
            properties,
            "/extended/instanceView/powerState/displayStatus",
        ) {
            return Some(state.trim_start_matches("VM ").to_string());
        }
    }
    maybe(properties, "/state")
        .or_else(|| maybe(properties, "/status"))
        .or_else(|| maybe(properties, "/powerState/code"))
        .or_else(|| maybe(properties, "/provisioningState"))
}

pub fn resource_of(row: &Value) -> Option<Resource> {
    let id = maybe(row, "/id")?;
    let kind = text(row, "/type").to_lowercase();
    Some(Resource {
        name: maybe(row, "/name").unwrap_or_else(|| client::short(&id)),
        label: label_of(&kind),
        group: maybe(row, "/resourceGroup").unwrap_or_else(|| group_of(&id)),
        location: text(row, "/location"),
        state: state_of(at(row, "/properties"), &kind),
        sku: maybe(row, "/sku/name").or_else(|| maybe(row, "/sku/tier")),
        tags: tags_of(row, "/tags"),
        kind,
        id,
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ListResources {
    pub subscription: Option<String>,
    /// Only this resource group's resources.
    pub group: Option<String>,
    /// Only resources of this type, such as microsoft.web/sites.
    pub kind: Option<String>,
    /// Words the name must contain.
    pub find: Option<String>,
    pub limit: Option<u32>,
}

/// The Resource Graph query a listing asks, with every value quoted into it checked first.
pub fn query_of(input: &ListResources) -> AzureResult<String> {
    let mut query = String::from("Resources");
    if let Some(group) = input.group.as_deref().filter(|group| !group.is_empty()) {
        let group = client::segment("resource group", group)?.to_lowercase();
        query.push_str(&format!(" | where resourceGroup =~ '{group}'"));
    }
    if let Some(kind) = input.kind.as_deref().filter(|kind| !kind.is_empty()) {
        let kind = kind.trim().to_lowercase();
        if !kind
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-'))
        {
            return Err(AzureError::BadArg(format!(
                "`{kind}` is not a resource type"
            )));
        }
        query.push_str(&format!(" | where type =~ '{kind}'"));
    }
    if let Some(find) = input
        .find
        .as_deref()
        .map(str::trim)
        .filter(|find| !find.is_empty())
    {
        if find.len() > 100 || find.contains('\'') || find.contains('\\') {
            return Err(AzureError::BadArg(
                "those words can't be searched for".into(),
            ));
        }
        query.push_str(&format!(" | where name contains '{find}'"));
    }
    query.push_str(" | project id, name, type, resourceGroup, location, sku, tags, properties");
    query.push_str(" | order by resourceGroup asc, name asc");
    query.push_str(&format!(
        " | limit {}",
        input.limit.unwrap_or(GRAPH_LIMIT).clamp(1, GRAPH_LIMIT)
    ));
    Ok(query)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub resources: Vec<Resource>,
    /// How many of each type there are, most first.
    pub kinds: Vec<Count>,
    pub truncated: bool,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub kind: String,
    pub label: String,
    pub count: usize,
}

pub fn counts_of(resources: &[Resource]) -> Vec<Count> {
    let mut counts: Vec<Count> = Vec::new();
    for resource in resources {
        match counts.iter_mut().find(|count| count.kind == resource.kind) {
            Some(count) => count.count += 1,
            None => counts.push(Count {
                kind: resource.kind.clone(),
                label: resource.label.clone(),
                count: 1,
            }),
        }
    }
    counts.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    counts
}

/// Asks Resource Graph for the subscription's resources, in one call.
pub async fn resources(input: ListResources) -> AzureResult<Inventory> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let query = query_of(&input)?;
    let answer = client::post(
        GRAPH_PATH,
        GRAPH_API,
        &json!({ "subscriptions": [subscription], "query": query }),
    )
    .await?;
    let resources: Vec<Resource> = at(&answer, "/data")
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(resource_of)
        .collect();
    Ok(Inventory {
        kinds: counts_of(&resources),
        truncated: at(&answer, "/resultTruncated").as_str() == Some("true"),
        resources,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn vm_row() -> Value {
        json!({
            "id": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Compute/virtualMachines/nine-mills",
            "name": "nine-mills",
            "type": "microsoft.compute/virtualmachines",
            "resourceGroup": "nine-mills",
            "location": "uaenorth",
            "sku": { "name": "Standard_B1s" },
            "tags": { "env": "prod", "owner": "ankit" },
            "properties": { "provisioningState": "Succeeded", "extended": { "instanceView": { "powerState": { "displayStatus": "VM running" } } } }
        })
    }

    #[test]
    fn a_resource_reads_its_name_group_state_and_tags() {
        let resource = resource_of(&vm_row()).expect("reads");
        assert_eq!(resource.label, "Virtual machine");
        assert_eq!(resource.group, "nine-mills");
        assert_eq!(resource.state.as_deref(), Some("running"));
        assert_eq!(resource.sku.as_deref(), Some("Standard_B1s"));
        assert_eq!(
            resource.tags,
            [
                ("env".into(), "prod".into()),
                ("owner".into(), "ankit".into())
            ]
        );
        assert!(resource_of(&json!({ "name": "no id" })).is_none());
    }

    #[test]
    fn a_type_reads_as_what_people_call_it() {
        assert_eq!(label_of("Microsoft.Web/sites"), "Web app");
        assert_eq!(label_of("microsoft.keyvault/vaults"), "Key vault");
        assert_eq!(label_of("microsoft.signalrservice/signalr"), "Signalr");
    }

    #[test]
    fn the_graph_query_narrows_only_by_what_was_checked() {
        let all = query_of(&ListResources::default()).expect("valid");
        assert!(all.starts_with("Resources | project"));
        assert!(all.ends_with("| limit 1000"));
        let narrow = query_of(&ListResources {
            group: Some("Nine-Mills".into()),
            kind: Some("Microsoft.Compute/virtualMachines".into()),
            find: Some("mills".into()),
            limit: Some(5000),
            ..ListResources::default()
        })
        .expect("valid");
        assert!(narrow.contains("where resourceGroup =~ 'nine-mills'"));
        assert!(narrow.contains("where type =~ 'microsoft.compute/virtualmachines'"));
        assert!(narrow.contains("where name contains 'mills'"));
        assert!(narrow.ends_with("| limit 1000"));
        assert!(query_of(&ListResources {
            group: Some("a' or '1'=='1".into()),
            ..ListResources::default()
        })
        .is_err());
        assert!(query_of(&ListResources {
            find: Some("it's".into()),
            ..ListResources::default()
        })
        .is_err());
        assert!(query_of(&ListResources {
            kind: Some("web | project 1".into()),
            ..ListResources::default()
        })
        .is_err());
    }

    #[test]
    fn types_are_counted_with_the_most_first() {
        let resources = vec![
            resource_of(&vm_row()).expect("reads"),
            resource_of(&json!({ "id": "/subscriptions/s1/resourceGroups/g/providers/Microsoft.Web/sites/a", "type": "microsoft.web/sites" })).expect("reads"),
            resource_of(&json!({ "id": "/subscriptions/s1/resourceGroups/g/providers/Microsoft.Web/sites/b", "type": "microsoft.web/sites" })).expect("reads"),
        ];
        assert_eq!(
            counts_of(&resources),
            [
                Count {
                    kind: "microsoft.web/sites".into(),
                    label: "Web app".into(),
                    count: 2
                },
                Count {
                    kind: "microsoft.compute/virtualmachines".into(),
                    label: "Virtual machine".into(),
                    count: 1
                }
            ]
        );
    }

    #[test]
    fn a_resource_group_reads_its_location_and_tags() {
        let group = group_of_value(&json!({
            "name": "nine-mills", "location": "uaenorth",
            "properties": { "provisioningState": "Succeeded" }, "tags": { "env": "prod" }
        }))
        .expect("reads");
        assert_eq!(
            group,
            Group {
                name: "nine-mills".into(),
                location: "uaenorth".into(),
                state: "Succeeded".into(),
                tags: vec![("env".into(), "prod".into())]
            }
        );
    }
}
