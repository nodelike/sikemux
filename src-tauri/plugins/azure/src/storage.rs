// Storage accounts with their blob containers, and Key Vault secrets by name.
// No key, connection string or secret value is ever read.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{self, at, maybe, text};
use crate::error::{AzureError, AzureResult};

const STORAGE_API: &str = "2024-01-01";
const VAULT_API: &str = "2024-11-01";

async fn subscription_of(named: Option<&str>) -> AzureResult<String> {
    if let Some(named) = named.filter(|named| !named.is_empty()) {
        return client::segment("subscription", named);
    }
    crate::auth::status()
        .await
        .current
        .map(|subscription| subscription.id)
        .ok_or(AzureError::Unconfigured)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub group: String,
    pub location: String,
    /// Standard_LRS, Premium_ZRS and the like.
    pub sku: String,
    /// Hot, Cool or Cold.
    pub tier: Option<String>,
    pub kind: String,
    /// Whether anything in it may be read without signing in.
    pub public_access: bool,
    pub https_only: bool,
    pub created: Option<String>,
}

pub fn account_of(account: &Value) -> Option<Account> {
    let id = maybe(account, "/id")?;
    Some(Account {
        name: maybe(account, "/name").unwrap_or_else(|| client::short(&id)),
        group: client::group_of(&id),
        location: text(account, "/location"),
        sku: text(account, "/sku/name"),
        tier: maybe(account, "/properties/accessTier"),
        kind: text(account, "/kind"),
        public_access: at(account, "/properties/allowBlobPublicAccess")
            .as_bool()
            .unwrap_or(false),
        https_only: at(account, "/properties/supportsHttpsTrafficOnly")
            .as_bool()
            .unwrap_or(true),
        created: maybe(account, "/properties/creationTime"),
        id,
    })
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct InSubscription {
    pub subscription: Option<String>,
    pub group: Option<String>,
}

pub async fn accounts(input: InSubscription) -> AzureResult<Vec<Account>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let path = match input.group.as_deref().filter(|group| !group.is_empty()) {
        Some(group) => format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Storage/storageAccounts",
            client::segment("resource group", group)?
        ),
        None => format!("/subscriptions/{subscription}/providers/Microsoft.Storage/storageAccounts"),
    };
    let listed = client::list(&path, STORAGE_API, &[]).await?;
    let mut accounts: Vec<Account> = listed.iter().filter_map(account_of).collect();
    accounts.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    Ok(accounts)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub name: String,
    /// None, Blob or Container; anything but None can be read without signing in.
    pub public_access: String,
    pub last_changed: Option<String>,
    pub leased: bool,
}

pub fn container_of(container: &Value) -> Option<Container> {
    Some(Container {
        name: maybe(container, "/name")?,
        public_access: maybe(container, "/properties/publicAccess")
            .unwrap_or_else(|| "None".into()),
        last_changed: maybe(container, "/properties/lastModifiedTime"),
        leased: maybe(container, "/properties/leaseStatus").as_deref() == Some("Locked"),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRef {
    #[serde(default)]
    pub subscription: Option<String>,
    pub group: String,
    pub name: String,
}

pub async fn containers(input: AccountRef) -> AzureResult<Vec<Container>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let path = format!(
        "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.Storage/storageAccounts/{}/blobServices/default/containers",
        client::segment("resource group", &input.group)?,
        client::segment("storage account", &input.name)?
    );
    let listed = client::list(&path, STORAGE_API, &[]).await?;
    let mut containers: Vec<Container> = listed.iter().filter_map(container_of).collect();
    containers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(containers)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Vault {
    pub id: String,
    pub name: String,
    pub group: String,
    pub location: String,
    /// The address its secrets are read from.
    pub uri: Option<String>,
    /// Whether a deleted vault can be brought back.
    pub soft_delete: bool,
    pub purge_protection: bool,
    /// Access policies or Azure RBAC.
    pub rbac: bool,
}

pub fn vault_of(vault: &Value) -> Option<Vault> {
    let id = maybe(vault, "/id")?;
    Some(Vault {
        name: maybe(vault, "/name").unwrap_or_else(|| client::short(&id)),
        group: client::group_of(&id),
        location: text(vault, "/location"),
        uri: maybe(vault, "/properties/vaultUri"),
        soft_delete: at(vault, "/properties/enableSoftDelete")
            .as_bool()
            .unwrap_or(false),
        purge_protection: at(vault, "/properties/enablePurgeProtection")
            .as_bool()
            .unwrap_or(false),
        rbac: at(vault, "/properties/enableRbacAuthorization")
            .as_bool()
            .unwrap_or(false),
        id,
    })
}

/// Key vaults in the subscription. Only the vaults are listed; no secret is read.
pub async fn vaults(input: InSubscription) -> AzureResult<Vec<Vault>> {
    let subscription = subscription_of(input.subscription.as_deref()).await?;
    let path = match input.group.as_deref().filter(|group| !group.is_empty()) {
        Some(group) => format!(
            "/subscriptions/{subscription}/resourceGroups/{}/providers/Microsoft.KeyVault/vaults",
            client::segment("resource group", group)?
        ),
        None => format!("/subscriptions/{subscription}/providers/Microsoft.KeyVault/vaults"),
    };
    let listed = client::list(&path, VAULT_API, &[]).await?;
    let mut vaults: Vec<Vault> = listed.iter().filter_map(vault_of).collect();
    vaults.sort_by(|a, b| (&a.group, &a.name).cmp(&(&b.group, &b.name)));
    Ok(vaults)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_storage_account_reads_its_sku_tier_and_whether_it_is_open() {
        let account = account_of(&json!({
            "id": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.Storage/storageAccounts/millsdata",
            "name": "millsdata", "location": "uaenorth", "kind": "StorageV2",
            "sku": { "name": "Standard_LRS" },
            "properties": { "accessTier": "Hot", "allowBlobPublicAccess": true, "supportsHttpsTrafficOnly": true, "creationTime": "2026-01-02T10:00:00Z" }
        }))
        .expect("reads");
        assert_eq!(account.sku, "Standard_LRS");
        assert_eq!(account.tier.as_deref(), Some("Hot"));
        assert!(account.public_access);
        assert_eq!(account.group, "nine-mills");
    }

    #[test]
    fn a_container_says_whether_it_can_be_read_without_signing_in() {
        let open = container_of(&json!({ "name": "public", "properties": { "publicAccess": "Blob", "leaseStatus": "Unlocked" } })).expect("reads");
        assert_eq!(open.public_access, "Blob");
        assert!(!open.leased);
        let shut =
            container_of(&json!({ "name": "backups", "properties": { "leaseStatus": "Locked" } }))
                .expect("reads");
        assert_eq!(shut.public_access, "None");
        assert!(shut.leased);
    }

    #[test]
    fn a_vault_reads_its_address_and_protections_but_no_secret() {
        let vault = vault_of(&json!({
            "id": "/subscriptions/s1/resourceGroups/nine-mills/providers/Microsoft.KeyVault/vaults/mills-kv",
            "name": "mills-kv", "location": "uaenorth",
            "properties": { "vaultUri": "https://mills-kv.vault.azure.net/", "enableSoftDelete": true, "enableRbacAuthorization": true }
        }))
        .expect("reads");
        assert_eq!(
            vault.uri.as_deref(),
            Some("https://mills-kv.vault.azure.net/")
        );
        assert!(vault.soft_delete && vault.rbac && !vault.purge_protection);
    }
}
