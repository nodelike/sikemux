// A project's firewall: the rules and IP blocks it applies, and Attack Challenge
// Mode, which makes every visitor pass a browser check first.

use std::path::Path;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{at, text, Session};
use crate::error::{VercelError, VercelResult};
use crate::projects::{segment, ProjectRef};

/// What a rule does to a request it matches, as the dashboard words it.
fn action_of(rule: &Value) -> String {
    let action = text(rule, "/action/mitigate/action");
    match action.as_str() {
        "deny" => "Deny".into(),
        "challenge" => "Challenge".into(),
        "log" => "Log".into(),
        "bypass" => "Bypass".into(),
        "rate_limit" => "Rate limit".into(),
        "redirect" => "Redirect".into(),
        "" => "—".into(),
        other => other.to_string(),
    }
}

pub fn firewall_of(config: &Value, project: &Value) -> Value {
    let rules: Vec<Value> = at(config, "/rules")
        .as_array()
        .into_iter()
        .flatten()
        .map(|rule| {
            json!({
                "id": at(rule, "/id"),
                "name": at(rule, "/name"),
                "description": at(rule, "/description"),
                "active": at(rule, "/active").as_bool().unwrap_or(true),
                "action": action_of(rule),
            })
        })
        .collect();
    let blocks: Vec<Value> = at(config, "/ips")
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| {
            json!({
                "ip": at(block, "/ip"),
                "hostname": at(block, "/hostname"),
                "note": at(block, "/notes"),
                "action": at(block, "/action"),
            })
        })
        .collect();
    let security = at(project, "/security");
    let until = at(security, "/attackModeActiveUntil").as_i64();
    json!({
        "enabled": at(config, "/firewallEnabled").as_bool().unwrap_or(false),
        "rules": rules,
        "blocks": blocks,
        "attackMode": at(security, "/attackModeEnabled").as_bool().unwrap_or(false),
        "attackModeUntil": until,
    })
}

pub async fn firewall(data_dir: &Path, input: ProjectRef) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    let id = text(&read, "/id");
    let config = match session
        .get("/v1/security/firewall/config/active", &[("projectId", id)])
        .await
    {
        Ok(config) => config,
        Err(VercelError::NotFound(_)) => Value::Null,
        Err(other) => return Err(other),
    };
    Ok(firewall_of(&config, &read))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttackMode {
    pub project: String,
    #[serde(default)]
    pub team: Option<String>,
    pub enabled: bool,
    /// How long to keep it on, in hours; it stays on until turned off without one.
    #[serde(default)]
    pub hours: Option<u32>,
}

pub fn attack_mode_body(project_id: &str, input: &AttackMode, now_ms: i64) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("projectId".into(), json!(project_id));
    body.insert("attackModeEnabled".into(), json!(input.enabled));
    if let (true, Some(hours)) = (input.enabled, input.hours.filter(|hours| *hours > 0)) {
        body.insert(
            "attackModeActiveUntil".into(),
            json!(now_ms + i64::from(hours) * 3_600_000),
        );
    }
    Value::Object(body)
}

pub async fn set_attack_mode(data_dir: &Path, input: AttackMode) -> VercelResult<Value> {
    let project = segment("project", &input.project)?;
    let session = Session::of(data_dir, input.team.as_deref()).await?;
    let read = session.get(&format!("/v9/projects/{project}"), &[]).await?;
    let id = text(&read, "/id");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0));
    session
        .send(
            Method::POST,
            "/v1/security/attack-mode",
            &[],
            Some(attack_mode_body(&id, &input, now)),
        )
        .await?;
    firewall(
        data_dir,
        ProjectRef {
            project,
            team: input.team,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_blocks_read_as_the_dashboard_words_them() {
        let config = json!({
            "firewallEnabled": true,
            "rules": [
                { "id": "rule_1", "name": "Block wp-admin", "description": "Bots probing WordPress", "active": true, "action": { "mitigate": { "action": "deny" } } },
                { "id": "rule_2", "name": "Slow the API", "active": false, "action": { "mitigate": { "action": "rate_limit" } } }
            ],
            "ips": [{ "id": "ip_1", "ip": "203.0.113.7", "hostname": "shop.acme.dev", "notes": "scraper", "action": "deny" }]
        });
        let project = json!({ "security": { "attackModeEnabled": true, "attackModeActiveUntil": 1792400000000_i64 } });
        let read = firewall_of(&config, &project);
        assert_eq!(read["enabled"], true);
        assert_eq!(read["rules"][0]["action"], "Deny");
        assert_eq!(read["rules"][1]["active"], false);
        assert_eq!(read["rules"][1]["action"], "Rate limit");
        assert_eq!(read["blocks"][0]["ip"], "203.0.113.7");
        assert_eq!(read["attackMode"], true);
        let none = firewall_of(&Value::Null, &json!({}));
        assert_eq!(
            (none["rules"].clone(), none["attackMode"].clone()),
            (json!([]), json!(false))
        );
    }

    #[test]
    fn attack_mode_can_be_turned_on_for_some_hours() {
        let on = AttackMode {
            project: "web".into(),
            team: None,
            enabled: true,
            hours: Some(2),
        };
        assert_eq!(
            attack_mode_body("prj_1", &on, 1_000),
            json!({ "projectId": "prj_1", "attackModeEnabled": true, "attackModeActiveUntil": 7_201_000 })
        );
        let off = AttackMode {
            project: "web".into(),
            team: None,
            enabled: false,
            hours: Some(2),
        };
        assert_eq!(
            attack_mode_body("prj_1", &off, 1_000),
            json!({ "projectId": "prj_1", "attackModeEnabled": false })
        );
    }
}
