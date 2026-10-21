// VPC network: networks, subnets in every region, and firewall rules with
// what they let through and from where, which answer why something cannot
// be reached; a rule can be turned off or on.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, short, text};
use crate::compute::operation_of;
use crate::error::GcpResult;

const API: &str = "https://compute.googleapis.com/compute/v1";

pub fn network(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "mode": if at(value, "/autoCreateSubnetworks").as_bool() == Some(true) { "auto" } else { "custom" },
        "routing": at(value, "/routingConfig/routingMode"),
        "mtu": at(value, "/mtu"),
        "subnets": at(value, "/subnetworks").as_array().map(|subnets| subnets.len()).unwrap_or(0),
        "createdAt": at(value, "/creationTimestamp"),
    })
}

pub fn subnet(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "region": short(&text(value, "/region")),
        "network": short(&text(value, "/network")),
        "range": at(value, "/ipCidrRange"),
        "secondary": at(value, "/secondaryIpRanges").as_array().into_iter().flatten().map(|range| format!("{} {}", text(range, "/rangeName"), text(range, "/ipCidrRange"))).collect::<Vec<_>>(),
        "privateGoogleAccess": at(value, "/privateIpGoogleAccess").as_bool() == Some(true),
        "purpose": at(value, "/purpose"),
    })
}

fn rules_of(rules: &Value) -> Vec<String> {
    rules
        .as_array()
        .into_iter()
        .flatten()
        .map(|rule| {
            let protocol = text(rule, "/IPProtocol");
            let ports: Vec<String> = at(rule, "/ports")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if ports.is_empty() {
                protocol
            } else {
                format!("{protocol}:{}", ports.join(","))
            }
        })
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// A firewall rule as the pane and agents read it: which way, what it allows or denies, from where to what.
pub fn firewall(value: &Value) -> Value {
    let denied = rules_of(at(value, "/denied"));
    let tagged = |path: &str| {
        strings(at(value, path))
            .into_iter()
            .map(|tag| format!("tag:{tag}"))
            .collect::<Vec<_>>()
    };
    let sources = [
        strings(at(value, "/sourceRanges")),
        tagged("/sourceTags"),
        strings(at(value, "/sourceServiceAccounts")),
    ]
    .concat();
    let targets = [
        tagged("/targetTags"),
        strings(at(value, "/targetServiceAccounts")),
    ]
    .concat();
    json!({
        "name": at(value, "/name"),
        "network": short(&text(value, "/network")),
        "direction": at(value, "/direction"),
        "priority": at(value, "/priority"),
        "action": if denied.is_empty() { "allow" } else { "deny" },
        "protocols": if denied.is_empty() { rules_of(at(value, "/allowed")) } else { denied },
        "sources": sources,
        "destinations": strings(at(value, "/destinationRanges")),
        "targets": targets,
        "disabled": at(value, "/disabled").as_bool() == Some(true),
        "logging": at(value, "/logConfig/enable").as_bool() == Some(true),
        "description": at(value, "/description"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

/// The project's networks, their subnets in every region, and its firewall rules by priority.
pub async fn overview(input: Listing) -> GcpResult<Value> {
    let project = input.target.project()?;
    let (networks, subnets, firewalls) = futures::future::try_join3(
        client::list(
            &input.target,
            &format!("{API}/projects/{project}/global/networks"),
            &[("maxResults", "500".into())],
            "items",
            500,
        ),
        client::get(
            &input.target,
            &format!("{API}/projects/{project}/aggregated/subnetworks"),
            &[
                ("maxResults", "500".into()),
                ("returnPartialSuccess", "true".into()),
            ],
        ),
        client::list(
            &input.target,
            &format!("{API}/projects/{project}/global/firewalls"),
            &[("maxResults", "500".into())],
            "items",
            1000,
        ),
    )
    .await?;
    let mut subnets: Vec<Value> = at(&subnets, "/items")
        .as_object()
        .into_iter()
        .flat_map(|regions| regions.values())
        .flat_map(|region| {
            at(region, "/subnetworks")
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|raw| subnet(&raw))
        .collect();
    subnets.sort_by_key(|each| (text(each, "/network"), text(each, "/region")));
    let mut firewalls: Vec<Value> = firewalls.iter().map(firewall).collect();
    firewalls.sort_by_key(|each| {
        (
            at(each, "/priority").as_u64().unwrap_or(1000),
            text(each, "/name"),
        )
    });
    Ok(
        json!({ "networks": networks.iter().map(network).collect::<Vec<_>>(), "subnets": subnets, "firewalls": firewalls }),
    )
}

#[derive(Deserialize)]
pub struct FirewallChange {
    #[serde(flatten)]
    pub target: Target,
    pub rule: String,
    pub disabled: bool,
}

/// Turns a firewall rule off, or back on.
pub async fn set_firewall(input: FirewallChange) -> GcpResult<Value> {
    let project = input.target.project()?;
    let url = format!(
        "{API}/projects/{project}/global/firewalls/{}",
        segment("firewall rule", &input.rule)?
    );
    let operation = client::patch(
        &input.target,
        &url,
        &[],
        &json!({ "disabled": input.disabled }),
    )
    .await?;
    Ok(operation_of(&operation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_firewall_rule_reads_as_what_it_lets_through() {
        let read = firewall(&json!({
            "name": "allow-ssh", "network": "projects/acme/global/networks/default", "direction": "INGRESS", "priority": 1000,
            "allowed": [{ "IPProtocol": "tcp", "ports": ["22", "3389"] }, { "IPProtocol": "icmp" }],
            "sourceRanges": ["35.235.240.0/20"], "targetTags": ["bastion"], "logConfig": { "enable": true }
        }));
        assert_eq!(read["action"], "allow");
        assert_eq!(read["protocols"], json!(["tcp:22,3389", "icmp"]));
        assert_eq!(read["sources"], json!(["35.235.240.0/20"]));
        assert_eq!(read["targets"], json!(["tag:bastion"]));
        assert_eq!(read["network"], "default");
        let deny = firewall(
            &json!({ "name": "deny-db", "denied": [{ "IPProtocol": "tcp", "ports": ["5432"] }], "sourceTags": ["web"] }),
        );
        assert_eq!(deny["action"], "deny");
        assert_eq!(deny["sources"], json!(["tag:web"]));
    }

    #[test]
    fn networks_and_subnets_read_plainly() {
        assert_eq!(
            network(
                &json!({ "name": "default", "autoCreateSubnetworks": true, "subnetworks": ["a", "b"] })
            )["mode"],
            "auto"
        );
        let read = subnet(
            &json!({ "name": "prod-us", "region": "https://.../regions/us-central1", "network": "https://.../networks/prod", "ipCidrRange": "10.20.0.0/20", "privateIpGoogleAccess": true, "secondaryIpRanges": [{ "rangeName": "pods", "ipCidrRange": "10.4.0.0/14" }] }),
        );
        assert_eq!(read["region"], "us-central1");
        assert_eq!(read["secondary"], json!(["pods 10.4.0.0/14"]));
    }
}
