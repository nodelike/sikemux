// Who is signed in to the Azure CLI, the subscriptions it can see, and the access
// tokens calls use. Tokens live only in memory and come from the CLI itself.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli;
use crate::client::{at, text};
use crate::error::{AzureError, AzureResult};

/// Azure's access tokens last an hour; one is asked for again well before then.
const TOKEN_TTL: Duration = Duration::from_secs(45 * 60);

static TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

pub fn forget_token() {
    if let Ok(mut held) = TOKEN.lock() {
        *held = None;
    }
}

/// An access token for the Resource Manager, from memory or freshly from the CLI.
pub async fn token() -> AzureResult<String> {
    if let Ok(held) = TOKEN.lock() {
        if let Some((token, _)) = held.as_ref().filter(|(_, at)| at.elapsed() < TOKEN_TTL) {
            return Ok(token.clone());
        }
    }
    let answer = cli::json(&[
        "account",
        "get-access-token",
        "--resource=https://management.azure.com/",
    ])
    .await?;
    let token = text(&answer, "/accessToken");
    if token.is_empty() || token.contains(char::is_whitespace) {
        return Err(AzureError::Auth(
            "the Azure CLI gave no access token".into(),
        ));
    }
    if let Ok(mut held) = TOKEN.lock() {
        *held = Some((token.clone(), Instant::now()));
    }
    Ok(token)
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub tenant_id: String,
    /// Enabled, Disabled, Warned, PastDue or Expired.
    pub state: String,
    pub is_default: bool,
    /// Who is signed in to it, such as an email or a service principal's id.
    pub user: String,
}

pub fn subscriptions_of(listed: &Value) -> Vec<Subscription> {
    let mut subscriptions: Vec<Subscription> = listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let id = text(entry, "/id");
            (!id.is_empty()).then(|| Subscription {
                id,
                name: text(entry, "/name"),
                tenant_id: text(entry, "/tenantId"),
                state: text(entry, "/state"),
                is_default: at(entry, "/isDefault").as_bool().unwrap_or(false),
                user: text(entry, "/user/name"),
            })
        })
        .collect();
    subscriptions.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
    subscriptions
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The CLI is installed and an account is signed in.
    pub ready: bool,
    /// Alias of `ready`, which the pane's sign-in state reads.
    pub configured: bool,
    /// The sign-in has run out and the person must sign in again.
    pub auth_failed: bool,
    /// cli-missing, signed-out or auth when not ready.
    pub category: Option<String>,
    pub message: Option<String>,
    pub subscriptions: Vec<Subscription>,
    /// The subscription calls use when none is named.
    pub current: Option<Subscription>,
    pub user: String,
    pub cli_version: Option<String>,
}

fn version_of(answer: &Value) -> Option<String> {
    at(answer, "/azure-cli")
        .as_str()
        .map(str::to_string)
        .filter(|version| !version.is_empty())
}

pub async fn status() -> Status {
    let base = |category: &str, message: String| Status {
        ready: false,
        configured: false,
        auth_failed: category == "auth",
        category: Some(category.to_string()),
        message: Some(message),
        subscriptions: Vec::new(),
        current: None,
        user: String::new(),
        cli_version: None,
    };
    let version = cli::json(&["version"]).await;
    let cli_version = match &version {
        Ok(answer) => version_of(answer),
        Err(AzureError::NoCli) => return base("cli-missing", AzureError::NoCli.to_string()),
        Err(_) => None,
    };
    let subscriptions = match cli::json(&["account", "list", "--all"]).await {
        Ok(listed) => subscriptions_of(&listed),
        Err(AzureError::NoCli) => return base("cli-missing", AzureError::NoCli.to_string()),
        Err(error @ AzureError::Unconfigured) => {
            let mut status = base("signed-out", error.to_string());
            status.cli_version = cli_version;
            return status;
        }
        Err(error) => {
            let mut status = base(
                if matches!(error, AzureError::Auth(_)) {
                    "auth"
                } else {
                    "azure"
                },
                error.to_string(),
            );
            status.cli_version = cli_version;
            return status;
        }
    };
    if subscriptions.is_empty() {
        let mut status = base("signed-out", AzureError::Unconfigured.to_string());
        status.cli_version = cli_version;
        return status;
    }
    let current = subscriptions
        .iter()
        .find(|subscription| subscription.is_default)
        .or_else(|| subscriptions.first())
        .cloned();
    Status {
        ready: true,
        configured: true,
        auth_failed: false,
        category: None,
        message: None,
        user: current
            .as_ref()
            .map(|subscription| subscription.user.clone())
            .unwrap_or_default(),
        current,
        subscriptions,
        cli_version,
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SignInWith {
    /// The tenant to sign in to, when the person belongs to more than one.
    pub tenant: Option<String>,
}

/// Opens the browser sign-in the CLI uses, and waits for the person to finish it.
pub async fn sign_in(input: SignInWith) -> AzureResult<Status> {
    let mut args = vec!["login".to_string()];
    if let Some(tenant) = input.tenant.as_deref().filter(|tenant| !tenant.is_empty()) {
        args.push(format!("--tenant={}", cli::value(tenant)?));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    cli::sign_in(&refs).await?;
    forget_token();
    Ok(status().await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseSubscription {
    pub subscription: String,
}

/// Makes a subscription the one calls use when none is named.
pub async fn use_subscription(input: UseSubscription) -> AzureResult<Status> {
    let subscription = cli::value(&input.subscription)?;
    let out = cli::run(&["account", "set", &format!("--subscription={subscription}")]).await?;
    if !out.ok {
        return Err(cli::classify(&out.stderr));
    }
    forget_token();
    Ok(status().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscriptions_are_read_with_the_default_one_first() {
        let listed = json!([
            { "id": "11111111-2222-3333-4444-555555555555", "name": "Side projects", "tenantId": "t1", "state": "Enabled", "isDefault": false, "user": { "name": "ankit@acme.dev" } },
            { "id": "66666666-7777-8888-9999-000000000000", "name": "Acme production", "tenantId": "t1", "state": "Enabled", "isDefault": true, "user": { "name": "ankit@acme.dev" } },
            { "name": "no id" }
        ]);
        let subscriptions = subscriptions_of(&listed);
        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].name, "Acme production");
        assert!(subscriptions[0].is_default);
        assert_eq!(subscriptions[0].user, "ankit@acme.dev");
    }

    #[test]
    fn the_clis_version_is_read_from_its_own_answer() {
        assert_eq!(
            version_of(&json!({ "azure-cli": "2.90.0", "azure-cli-core": "2.90.0" })).as_deref(),
            Some("2.90.0")
        );
        assert_eq!(version_of(&json!({})), None);
    }
}
