// Cloud DNS, read only: managed zones and their record sets.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Target;
use crate::client::{self, at, segment, text};
use crate::error::GcpResult;

const API: &str = "https://dns.googleapis.com/dns/v1";

pub fn zone(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "dnsName": at(value, "/dnsName"),
        "visibility": value.get("visibility").and_then(Value::as_str).unwrap_or("public"),
        "dnssec": text(value, "/dnssecConfig/state") == "on",
        "nameServers": at(value, "/nameServers"),
        "description": at(value, "/description"),
    })
}

pub fn record(value: &Value) -> Value {
    json!({
        "name": at(value, "/name"),
        "type": at(value, "/type"),
        "ttl": at(value, "/ttl"),
        "data": at(value, "/rrdatas"),
    })
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(flatten)]
    pub target: Target,
}

pub async fn zones(input: Listing) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let listed = client::list(
        &input.target,
        &format!("{API}/projects/{project}/managedZones"),
        &[("maxResults", "500".into())],
        "managedZones",
        1000,
    )
    .await?;
    let mut read: Vec<Value> = listed.iter().map(zone).collect();
    read.sort_by_key(|each| text(each, "/dnsName"));
    Ok(read)
}

#[derive(Deserialize)]
pub struct ZoneRef {
    #[serde(flatten)]
    pub target: Target,
    pub zone: String,
}

/// A zone's records, by name then type.
pub async fn records(input: ZoneRef) -> GcpResult<Vec<Value>> {
    let project = input.target.project()?;
    let url = format!(
        "{API}/projects/{project}/managedZones/{}/rrsets",
        segment("zone", &input.zone)?
    );
    let listed = client::list(
        &input.target,
        &url,
        &[("maxResults", "1000".into())],
        "rrsets",
        5000,
    )
    .await?;
    let mut read: Vec<Value> = listed.iter().map(record).collect();
    read.sort_by_key(|each| (text(each, "/name"), text(each, "/type")));
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_and_records_read_plainly() {
        let read = zone(
            &json!({ "name": "acme-dev", "dnsName": "acme.dev.", "dnssecConfig": { "state": "on" }, "nameServers": ["ns-cloud-a1.googledomains.com."] }),
        );
        assert_eq!(
            (read["visibility"].as_str(), read["dnssec"].as_bool()),
            (Some("public"), Some(true))
        );
        let a = record(
            &json!({ "name": "shop.acme.dev.", "type": "A", "ttl": 300, "rrdatas": ["34.120.1.2"] }),
        );
        assert_eq!(a["data"], json!(["34.120.1.2"]));
    }
}
