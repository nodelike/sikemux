//! The vault as Settings → Connectors shows it, and what the person changes there: which
//! hosts and projects may use each sign-in, the recovery key, and each sign-in's use log.

use std::collections::BTreeSet;
use std::sync::Arc;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use crate::accounts::vault::VaultUses;

use super::super::account::api::as_host;
use super::super::hosts::{self, HostReach as Reach};
use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::{local, sync, Scope, Status};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShownStatus {
    /// This host is on no account.
    #[default]
    Off,
    /// The account's vault waits for a host that holds its key to let this one in.
    Waiting,
    Open,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownVault {
    pub status: ShownStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// How many of the account's hosts hold the vault's key.
    pub holders: usize,
    /// The recovery key of the vault this host made, until the person says they saved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    pub sign_ins: Vec<ShownSignIn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownSignIn {
    pub id: String,
    pub plugin: String,
    pub label: String,
    /// The host it was made on.
    pub origin: String,
    pub hosts: Vec<ShownHost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_use: Option<ShownUse>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownHost {
    pub key: String,
    pub name: String,
    pub this: bool,
    pub online: bool,
    pub allowed: bool,
    /// The projects its agents may use the sign-in in; none for every project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    pub refreshes: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShownUse {
    /// The agent, such as `@Otter`, or `Sikemux` for the host itself.
    pub actor: String,
    pub host: String,
    pub count: i64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
}

/// What the account calls a host, or the start of its key when this host does not know it.
pub(crate) fn host_name(core: &Core, key: &str) -> String {
    if core
        .remote
        .host_key()
        .is_some_and(|me| me.public().to_string() == key)
    {
        return core.notify.host_name();
    }
    hosts::rows(core)
        .into_iter()
        .find(|row| row.id.as_deref() == Some(key))
        .map(|row| row.name)
        .unwrap_or_else(|| key.get(..8).unwrap_or(key).to_owned())
}

pub(crate) fn vault(core: &Core) -> ShownVault {
    let rows = hosts::rows(core);
    let state = core.vault.state.locked();
    let me = state.me.clone().unwrap_or_default();
    let (status, failed) = match &state.status {
        Status::Off => (ShownStatus::Off, None),
        Status::Waiting => (ShownStatus::Waiting, None),
        Status::Open => (ShownStatus::Open, None),
        Status::Failed(why) => (ShownStatus::Failed, Some(why.clone())),
    };
    let hosts: BTreeSet<&String> = state.holders.iter().chain(&state.waiting).collect();
    let named = |key: &str| {
        if key == me {
            return (core.notify.host_name(), true);
        }
        rows.iter()
            .find(|row| row.id.as_deref() == Some(key))
            .map_or_else(
                || (key.get(..8).unwrap_or(key).to_owned(), false),
                |row| (row.name.clone(), row.reach == Reach::Connected),
            )
    };
    let sign_ins = state
        .entries
        .iter()
        .map(|(id, entry)| {
            let body = &entry.body;
            let mut shown_hosts: Vec<ShownHost> = hosts
                .iter()
                .chain(
                    body.access
                        .keys()
                        .filter(|key| !hosts.contains(key))
                        .collect::<Vec<_>>()
                        .iter(),
                )
                .map(|key| {
                    let (name, online) = named(key);
                    let scope = body.access.get(*key);
                    ShownHost {
                        key: (*key).clone(),
                        name,
                        this: **key == me,
                        online,
                        allowed: scope.is_some(),
                        projects: scope.and_then(|scope| scope.projects.clone()),
                        refreshes: body.refresher == **key,
                    }
                })
                .collect();
            shown_hosts.sort_by_key(|host| (!host.this, !host.allowed, host.name.to_lowercase()));
            ShownSignIn {
                id: id.clone(),
                plugin: body.plugin.clone(),
                label: body.sign_in.label.clone(),
                origin: named(&body.origin).0,
                hosts: shown_hosts,
                last_use: state.latest.get(id).map(|(actor, at)| ShownUse {
                    actor: actor.clone(),
                    host: core.notify.host_name(),
                    count: 1,
                    at: *at,
                }),
            }
        })
        .collect();
    ShownVault {
        status,
        failed,
        holders: state.holders.len(),
        recovery: state.recovery_text.clone(),
        sign_ins,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowParams {
    pub entry: String,
    pub host: String,
    #[serde(default)]
    pub projects: Option<Vec<String>>,
}

pub(crate) async fn allow(core: &Arc<Core>, params: AllowParams) -> CoreResult<()> {
    sync::set_access(core, &params.entry, |body| {
        body.access.insert(
            params.host,
            Scope {
                projects: params.projects,
            },
        );
    })
    .await
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeParams {
    pub entry: String,
    pub host: String,
}

/// Takes a host off a sign-in. Taking off the last host removes the sign-in from the vault.
pub(crate) async fn revoke(core: &Arc<Core>, params: RevokeParams) -> CoreResult<()> {
    sync::set_access(core, &params.entry, |body| {
        body.access.remove(&params.host);
        if body.refresher == params.host {
            if let Some(next) = body.access.keys().next() {
                body.refresher = next.clone();
                body.shared = None;
            }
        }
    })
    .await
}

pub(crate) async fn recovery_saved(core: &Arc<Core>) -> CoreResult<()> {
    let account = core.vault.state.locked().account.clone();
    if let Some(account) = account {
        blocking(move || local::forget_recovery(&account).map_err(CoreError::from)).await?;
    }
    core.vault.state.locked().recovery_text = None;
    super::announce(core);
    super::inbox_changed(core);
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct RecoverParams {
    pub key: String,
}

pub(crate) async fn recover(core: &Arc<Core>, params: RecoverParams) -> CoreResult<()> {
    sync::recover(core, &params.key).await
}

#[derive(Serialize, Deserialize)]
pub struct UsesParams {
    pub entry: String,
}

/// The sign-in's uses over the last 30 days, on every host, newest first.
pub(crate) async fn uses(core: &Arc<Core>, params: UsesParams) -> CoreResult<Vec<ShownUse>> {
    let read: VaultUses = as_host(core, Method::GET, "/v1/vault/uses", None).await?;
    Ok(read
        .uses
        .into_iter()
        .filter(|row| row.entry_id == params.entry)
        .map(|row| ShownUse {
            actor: row.actor,
            host: row
                .host_name
                .unwrap_or_else(|| host_name(core, &row.host_key)),
            count: row.count,
            at: time::OffsetDateTime::parse(
                &row.last_at,
                &time::format_description::well_known::Rfc3339,
            )
            .map(|at| u64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0))
            .unwrap_or(0),
        })
        .collect())
}
