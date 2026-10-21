// A project's domains: adding one, the DNS records it needs, verifying it,
// pointing it at a branch or redirecting it, and removing it.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::segment;

/// Vercel's address for a domain at the root, such as acme.dev.
const APEX_A: &str = "76.76.21.21";
/// Vercel's address for a subdomain, such as www.acme.dev.
const SUBDOMAIN_CNAME: &str = "cname.vercel-dns.com";
const REDIRECT_CODES: &[u16] = &[301, 302, 307, 308];

pub fn checked_domain(name: &str) -> VercelResult<String> {
    let name = name.trim().trim_end_matches('.').to_lowercase();
    let name = name
        .strip_prefix("https://")
        .or_else(|| name.strip_prefix("http://"))
        .unwrap_or(&name)
        .trim_end_matches('/')
        .to_string();
    let labels: Vec<&str> = name.split('.').collect();
    let fits = name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !fits {
        return Err(VercelError::BadArg(format!(
            "`{name}` is not a domain name"
        )));
    }
    Ok(name)
}

/// A domain at the root of its zone, as far as can be told without asking DNS:
/// two labels, or three when the last two are a country's second level such as co.uk.
pub fn is_apex(name: &str) -> bool {
    let labels: Vec<&str> = name.split('.').collect();
    match labels.as_slice() {
        [_, _] => true,
        [_, second, last] => {
            last.len() == 2 && ["co", "com", "org", "net", "ac", "gov"].contains(second)
        }
        _ => false,
    }
}

/// The record to add for the domain to reach Vercel: what Vercel recommends, else its usual one.
fn pointing_record(name: &str, config: &Value) -> Value {
    if is_apex(name) {
        let recommended = at(config, "/recommendedIPv4/0/value/0")
            .as_str()
            .unwrap_or(APEX_A);
        json!({ "type": "A", "name": "@", "value": recommended })
    } else {
        let recommended = at(config, "/recommendedCNAME/0/value")
            .as_str()
            .unwrap_or(SUBDOMAIN_CNAME)
            .trim_end_matches('.');
        let host = name.split('.').next().unwrap_or(name);
        json!({ "type": "CNAME", "name": host, "value": recommended })
    }
}

/// A domain with what is left to set up: the TXT records proving it is the person's,
/// and the record pointing it at Vercel while it is misconfigured.
pub fn detail_of(domain: &Value, config: &Value) -> Value {
    let name = text(domain, "/name");
    let verified = at(domain, "/verified").as_bool().unwrap_or(false);
    let misconfigured = at(config, "/misconfigured").as_bool().unwrap_or(false);
    let mut records: Vec<Value> = Vec::new();
    if !verified {
        records.extend(
            at(domain, "/verification")
                .as_array()
                .into_iter()
                .flatten()
                .map(|record| {
                    json!({
                        "type": at(record, "/type"),
                        "name": at(record, "/domain"),
                        "value": at(record, "/value"),
                        "why": "Proves the domain is yours",
                    })
                }),
        );
    }
    if misconfigured {
        let mut record = pointing_record(&name, config);
        if let Some(object) = record.as_object_mut() {
            object.insert("why".into(), json!("Points the domain at Vercel"));
        }
        records.push(record);
    }
    json!({
        "name": name,
        "verified": verified,
        "misconfigured": misconfigured,
        "redirect": at(domain, "/redirect"),
        "redirectStatusCode": at(domain, "/redirectStatusCode"),
        "gitBranch": at(domain, "/gitBranch"),
        "records": records,
    })
}

#[derive(Deserialize)]
pub struct DomainRef {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    pub domain: String,
}

async fn read(session: &Session, project: &str, domain: &str) -> VercelResult<Value> {
    let found = session
        .get(&format!("/v9/projects/{project}/domains/{domain}"), &[])
        .await?;
    let config = session
        .get(&format!("/v6/domains/{domain}/config"), &[])
        .await
        .unwrap_or(Value::Null);
    Ok(detail_of(&found, &config))
}

pub async fn detail(data_dir: &Path, input: DomainRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let domain = checked_domain(&input.domain)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    read(&session, &project, &domain).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    pub domain: String,
    /// Serve this branch's latest deployment instead of production; "" for production again.
    #[serde(default)]
    pub branch: Option<String>,
    /// Send visitors on to this domain instead; "" to stop redirecting.
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub redirect_status_code: Option<u16>,
}

/// The routing part of a domain, as Vercel is sent it; only what was named changes.
pub fn routing_of(input: &Change) -> VercelResult<Map<String, Value>> {
    let mut body = Map::new();
    if let Some(branch) = &input.branch {
        let branch = branch.trim();
        body.insert(
            "gitBranch".into(),
            if branch.is_empty() {
                Value::Null
            } else {
                json!(branch)
            },
        );
    }
    if let Some(redirect) = &input.redirect {
        if redirect.trim().is_empty() {
            body.insert("redirect".into(), Value::Null);
            body.insert("redirectStatusCode".into(), Value::Null);
        } else {
            let target = checked_domain(redirect)?;
            if target == checked_domain(&input.domain)? {
                return Err(VercelError::BadArg(
                    "a domain can't redirect to itself".into(),
                ));
            }
            let code = input.redirect_status_code.unwrap_or(308);
            if !REDIRECT_CODES.contains(&code) {
                return Err(VercelError::BadArg(
                    "a redirect is 301, 302, 307 or 308".into(),
                ));
            }
            body.insert("redirect".into(), json!(target));
            body.insert("redirectStatusCode".into(), json!(code));
        }
    }
    if body
        .get("gitBranch")
        .is_some_and(|branch| !branch.is_null())
        && body
            .get("redirect")
            .is_some_and(|redirect| !redirect.is_null())
    {
        return Err(VercelError::BadArg(
            "a domain either serves a branch or redirects, not both".into(),
        ));
    }
    Ok(body)
}

pub async fn add(data_dir: &Path, input: Change) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let domain = checked_domain(&input.domain)?;
    let mut body = routing_of(&input)?;
    body.retain(|_, value| !value.is_null());
    body.insert("name".into(), json!(domain));
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    session
        .send(
            Method::POST,
            &format!("/v10/projects/{project}/domains"),
            &[],
            Some(Value::Object(body)),
        )
        .await?;
    read(&session, &project, &domain).await
}

pub async fn update(data_dir: &Path, input: Change) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let domain = checked_domain(&input.domain)?;
    let body = routing_of(&input)?;
    if body.is_empty() {
        return Err(VercelError::BadArg(
            "name a branch or a redirect to change".into(),
        ));
    }
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    session
        .send(
            Method::PATCH,
            &format!("/v9/projects/{project}/domains/{domain}"),
            &[],
            Some(Value::Object(body)),
        )
        .await?;
    read(&session, &project, &domain).await
}

/// Asks Vercel to look at the DNS again, answering with what is still missing.
pub async fn verify(data_dir: &Path, input: DomainRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let domain = checked_domain(&input.domain)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let verified = session
        .send(
            Method::POST,
            &format!("/v9/projects/{project}/domains/{domain}/verify"),
            &[],
            None,
        )
        .await;
    match verified {
        Ok(_) | Err(VercelError::Api(_)) => read(&session, &project, &domain).await,
        Err(other) => Err(other),
    }
}

pub async fn remove(data_dir: &Path, input: DomainRef) -> VercelResult<()> {
    let project = segment("project", &input.project)?;
    let domain = checked_domain(&input.domain)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    session
        .send(
            Method::DELETE,
            &format!("/v9/projects/{project}/domains/{domain}"),
            &[],
            None,
        )
        .await?;
    Ok(())
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Action {
    Add,
    Update,
    Verify,
    Remove,
}

/// One call for agents: add, update, verify or remove.
pub async fn change(data_dir: &Path, input: Value) -> VercelResult<Value> {
    let action: Action = serde_json::from_value(at(&input, "/action").clone())
        .map_err(|_| VercelError::BadArg("action is add, update, verify or remove".into()))?;
    let bad = |error: serde_json::Error| VercelError::BadArg(error.to_string());
    match action {
        Action::Add => add(data_dir, serde_json::from_value(input).map_err(bad)?).await,
        Action::Update => update(data_dir, serde_json::from_value(input).map_err(bad)?).await,
        Action::Verify => verify(data_dir, serde_json::from_value(input).map_err(bad)?).await,
        Action::Remove => remove(data_dir, serde_json::from_value(input).map_err(bad)?)
            .await
            .map(|()| json!({ "removed": true })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(value: Value) -> Change {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_domain_is_read_as_people_paste_it_and_nothing_else_passes() {
        assert_eq!(
            checked_domain(" https://Shop.Acme.dev/ ").unwrap(),
            "shop.acme.dev"
        );
        assert_eq!(checked_domain("acme.dev.").unwrap(), "acme.dev");
        assert!(checked_domain("localhost").is_err());
        assert!(checked_domain("acme..dev").is_err());
        assert!(checked_domain("-acme.dev").is_err());
        assert!(checked_domain("acme.dev/../v2").is_err());
    }

    #[test]
    fn a_root_domain_is_told_from_a_subdomain() {
        assert!(is_apex("acme.dev"));
        assert!(is_apex("acme.co.uk"));
        assert!(!is_apex("www.acme.dev"));
        assert!(!is_apex("shop.acme.co.uk"));
    }

    #[test]
    fn an_unverified_misconfigured_domain_lists_the_records_it_needs() {
        let domain = json!({
            "name": "shop.acme.dev", "verified": false,
            "verification": [{ "type": "TXT", "domain": "_vercel.acme.dev", "value": "vc-domain-verify=shop.acme.dev,abc", "reason": "pending_domain_verification" }]
        });
        let config = json!({ "misconfigured": true, "recommendedCNAME": [{ "rank": 1, "value": "abc123.vercel-dns-017.com." }] });
        let read = detail_of(&domain, &config);
        assert_eq!(
            read["records"][0],
            json!({ "type": "TXT", "name": "_vercel.acme.dev", "value": "vc-domain-verify=shop.acme.dev,abc", "why": "Proves the domain is yours" })
        );
        assert_eq!(
            read["records"][1],
            json!({ "type": "CNAME", "name": "shop", "value": "abc123.vercel-dns-017.com", "why": "Points the domain at Vercel" })
        );
        let root = detail_of(
            &json!({ "name": "acme.dev", "verified": true }),
            &json!({ "misconfigured": true }),
        );
        assert_eq!(
            root["records"],
            json!([{ "type": "A", "name": "@", "value": APEX_A, "why": "Points the domain at Vercel" }])
        );
        let done = detail_of(
            &json!({ "name": "acme.dev", "verified": true }),
            &json!({ "misconfigured": false }),
        );
        assert_eq!(done["records"], json!([]));
    }

    #[test]
    fn a_domain_serves_a_branch_or_redirects_but_not_both() {
        let redirect = routing_of(&change(
            json!({ "project": "web", "domain": "www.acme.dev", "redirect": "acme.dev" }),
        ))
        .unwrap();
        assert_eq!(
            Value::Object(redirect),
            json!({ "redirect": "acme.dev", "redirectStatusCode": 308 })
        );
        let back = routing_of(&change(
            json!({ "project": "web", "domain": "www.acme.dev", "redirect": "", "branch": "" }),
        ))
        .unwrap();
        assert_eq!(
            Value::Object(back),
            json!({ "gitBranch": null, "redirect": null, "redirectStatusCode": null })
        );
        assert!(routing_of(&change(json!({ "project": "web", "domain": "a.acme.dev", "redirect": "acme.dev", "branch": "staging" }))).is_err());
        assert!(routing_of(&change(
            json!({ "project": "web", "domain": "acme.dev", "redirect": "acme.dev" })
        ))
        .is_err());
        assert!(routing_of(&change(json!({ "project": "web", "domain": "a.acme.dev", "redirect": "acme.dev", "redirectStatusCode": 200 }))).is_err());
    }
}
