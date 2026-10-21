// Who is signed in to gcloud, the configurations it keeps, and the access
// tokens calls use. Tokens live only in memory and come from gcloud itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{at, text};
use crate::error::{GcpError, GcpResult};
use crate::gcloud;

/// gcloud's access tokens last an hour; one is asked for again well before then.
const TOKEN_TTL: Duration = Duration::from_secs(50 * 60);

static TOKENS: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

/// Which account and project a call runs as. No account means gcloud's active one.
#[derive(Deserialize, Clone, Default, Debug)]
pub struct Target {
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

impl Target {
    pub fn account(&self) -> Option<&str> {
        self.account
            .as_deref()
            .map(str::trim)
            .filter(|account| !account.is_empty())
    }

    /// The project the call is for; most calls cannot do without one.
    pub fn project(&self) -> GcpResult<String> {
        let project = self
            .project
            .as_deref()
            .map(str::trim)
            .filter(|project| !project.is_empty())
            .ok_or_else(|| GcpError::BadArg("no project was named".into()))?;
        crate::client::segment("project", project)
    }
}

fn key(account: Option<&str>) -> String {
    account.unwrap_or("").to_string()
}

pub fn forget_token(account: Option<&str>) {
    if let Ok(mut held) = TOKENS.lock() {
        if let Some(tokens) = held.as_mut() {
            tokens.remove(&key(account));
        }
    }
}

fn forget_all() {
    if let Ok(mut held) = TOKENS.lock() {
        *held = None;
    }
}

/// An access token for the account, from memory or freshly from gcloud.
pub async fn token(account: Option<&str>) -> GcpResult<String> {
    if let Ok(held) = TOKENS.lock() {
        if let Some((token, at)) = held.as_ref().and_then(|tokens| tokens.get(&key(account))) {
            if at.elapsed() < TOKEN_TTL {
                return Ok(token.clone());
            }
        }
    }
    let mut args = vec!["auth", "print-access-token"];
    let flag;
    if let Some(account) = account {
        flag = format!("--account={}", gcloud::value(account)?);
        args.push(&flag);
    }
    let out = gcloud::run(&args).await?;
    if !out.ok {
        return Err(gcloud::classify(&out.stderr));
    }
    let token = out.stdout.trim().to_string();
    if token.is_empty() || token.contains(char::is_whitespace) {
        return Err(GcpError::Auth("gcloud gave no access token".into()));
    }
    if let Ok(mut held) = TOKENS.lock() {
        held.get_or_insert_with(HashMap::new)
            .insert(key(account), (token.clone(), Instant::now()));
    }
    Ok(token)
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub account: String,
    pub active: bool,
    /// Service accounts sign in with a key; people sign in through the browser.
    pub service_account: bool,
}

pub fn accounts_of(listed: &Value) -> Vec<Account> {
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let account = text(entry, "/account");
            (!account.is_empty()).then(|| Account {
                service_account: account.ends_with(".gserviceaccount.com"),
                active: text(entry, "/status") == "ACTIVE",
                account,
            })
        })
        .collect()
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub name: String,
    pub active: bool,
    pub account: Option<String>,
    pub project: Option<String>,
    pub region: Option<String>,
    pub zone: Option<String>,
}

pub fn configurations_of(listed: &Value) -> Vec<Configuration> {
    let some = |value: String| (!value.is_empty()).then_some(value);
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = text(entry, "/name");
            (!name.is_empty()).then(|| Configuration {
                active: at(entry, "/is_active").as_bool() == Some(true),
                account: some(text(entry, "/properties/core/account")),
                project: some(text(entry, "/properties/core/project")),
                region: some(text(entry, "/properties/compute/region")),
                zone: some(text(entry, "/properties/compute/zone")),
                name,
            })
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether the gcloud CLI could be found.
    pub installed: bool,
    pub accounts: Vec<Account>,
    pub configurations: Vec<Configuration>,
    /// Whether application-default credentials exist, which client libraries and agents' code use.
    pub application_default: bool,
    pub message: Option<String>,
}

/// gcloud's settings folder, where it keeps its accounts and configurations.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) =
        sikemux_process::user_environment::var("CLOUDSDK_CONFIG").filter(|dir| !dir.is_empty())
    {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("gcloud"))
}

/// Whether gcloud has signed anyone in here, read from whether its files exist, without opening them.
pub fn signed_in_here() -> bool {
    config_dir().is_some_and(|dir| {
        dir.join("credentials.db").exists() || dir.join("legacy_credentials").exists()
    })
}

pub async fn status() -> Status {
    let application_default =
        config_dir().is_some_and(|dir| dir.join("application_default_credentials.json").exists());
    let listed = futures::future::join(
        gcloud::json(&["auth", "list"]),
        gcloud::json(&["config", "configurations", "list"]),
    )
    .await;
    match listed {
        (Err(GcpError::NoGcloud), _) | (_, Err(GcpError::NoGcloud)) => Status {
            installed: false,
            accounts: Vec::new(),
            configurations: Vec::new(),
            application_default,
            message: Some(GcpError::NoGcloud.to_string()),
        },
        (accounts, configurations) => {
            let message = accounts
                .as_ref()
                .err()
                .or(configurations.as_ref().err())
                .map(ToString::to_string);
            Status {
                installed: true,
                accounts: accounts
                    .map(|listed| accounts_of(&listed))
                    .unwrap_or_default(),
                configurations: configurations
                    .map(|listed| configurations_of(&listed))
                    .unwrap_or_default(),
                application_default,
                message,
            }
        }
    }
}

/// Signs a person in through the browser, as `gcloud auth login` does.
pub async fn sign_in() -> GcpResult<()> {
    gcloud::sign_in(&["auth", "login", "--brief", "--quiet"]).await?;
    forget_all();
    Ok(())
}

/// Makes application-default credentials, the ones Google's client libraries and agents' code use.
pub async fn sign_in_application_default() -> GcpResult<()> {
    gcloud::sign_in(&["auth", "application-default", "login", "--quiet"]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gcloud_lists_its_accounts_and_configurations() {
        let accounts = json!([
            { "account": "ankit@example.com", "status": "ACTIVE" },
            { "account": "deployer@acme.iam.gserviceaccount.com", "status": "" },
            { "status": "" }
        ]);
        assert_eq!(
            accounts_of(&accounts),
            [
                Account {
                    account: "ankit@example.com".into(),
                    active: true,
                    service_account: false
                },
                Account {
                    account: "deployer@acme.iam.gserviceaccount.com".into(),
                    active: false,
                    service_account: true
                },
            ]
        );
        let configurations = json!([{
            "name": "default", "is_active": true,
            "properties": { "core": { "account": "ankit@example.com", "project": "acme-prod" }, "compute": { "region": "us-central1", "zone": "us-central1-a" } }
        }, { "name": "side", "is_active": false, "properties": {} }]);
        let read = configurations_of(&configurations);
        assert_eq!(read.len(), 2);
        assert_eq!(
            read.first().and_then(|c| c.project.as_deref()),
            Some("acme-prod")
        );
        assert_eq!(
            read.get(1).map(|c| (c.active, c.project.clone())),
            Some((false, None))
        );
    }

    #[test]
    fn a_call_needs_a_project_that_is_one_name() {
        let target = Target {
            account: None,
            project: Some(" acme-prod ".into()),
        };
        assert_eq!(target.project().ok().as_deref(), Some("acme-prod"));
        assert!(Target::default().project().is_err());
        assert!(Target {
            account: None,
            project: Some("a/../b".into())
        }
        .project()
        .is_err());
    }
}
