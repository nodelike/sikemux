//! The vault as Settings → Connectors shows it, and what the person changes there: which
//! hosts and projects may use each sign-in, the recovery key, and each sign-in's use log.

use crate::server::parts::Parts;
pub use sikemux_calls::vault::{AllowParams, ShownHost, ShownSignIn, ShownStatus, ShownVault};
pub use sikemux_calls::vault::{RecoverParams, RevokeParams, ShownUse, UsesParams};
use std::collections::BTreeSet;
use std::sync::Arc;

use reqwest::Method;
use sikemux_process::sync::Locked;

use crate::accounts::vault::VaultUses;

use super::super::account::api::as_host;
use super::super::hosts::{self, HostReach as Reach};
use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::{local, sync, Scope, State, Status};

/// What the account calls a host, and whether this host reaches it now, or the start of its
/// key when the account does not name it.
pub(super) fn named(core: &Core, state: &State, key: &str) -> (String, bool) {
    if state.me.as_deref() == Some(key) {
        return (core.notify().host_name(), true);
    }
    let linked = hosts::rows(core)
        .into_iter()
        .find(|row| row.id.as_deref() == Some(key));
    let name = state
        .hosts
        .get(key)
        .cloned()
        .or_else(|| linked.as_ref().map(|row| row.name.clone()))
        .unwrap_or_else(|| key.get(..8).unwrap_or(key).to_owned());
    (
        name,
        linked.is_some_and(|row| row.reach == Reach::Connected),
    )
}

pub(crate) fn host_name(core: &Core, key: &str) -> String {
    let state = core.vault().state.locked();
    named(core, &state, key).0
}

pub(crate) fn vault(core: &Core) -> ShownVault {
    let state = core.vault().state.locked();
    let me = state.me.clone().unwrap_or_default();
    let (status, failed) = match &state.status {
        Status::Off => (ShownStatus::Off, None),
        Status::Waiting => (ShownStatus::Waiting, None),
        Status::Open => (ShownStatus::Open, None),
        Status::Failed(why) => (ShownStatus::Failed, Some(why.clone())),
    };
    let hosts: BTreeSet<&String> = state.holders.iter().chain(&state.waiting).collect();
    let named = |key: &str| named(core, &state, key);
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
                    host: core.notify().host_name(),
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
    let account = core.vault().state.locked().account.clone();
    if let Some(account) = account {
        blocking(move || local::forget_recovery(&account).map_err(CoreError::from)).await?;
    }
    core.vault().state.locked().recovery_text = None;
    super::announce(core);
    super::inbox_changed(core);
    Ok(())
}

pub(crate) async fn recover(core: &Arc<Core>, params: RecoverParams) -> CoreResult<()> {
    sync::recover(core, &params.key).await
}

/// The sign-in's uses over the last 30 days, on every host, newest first.
pub(crate) async fn uses(core: &Arc<Core>, params: UsesParams) -> CoreResult<Vec<ShownUse>> {
    if params.entry.len() != 32
        || !params
            .entry
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("that is not a vault entry's id".into());
    }
    let path = format!("/v1/vault/uses?entry={}", params.entry);
    let read: VaultUses = as_host(core, Method::GET, &path, None).await?;
    Ok(read
        .uses
        .into_iter()
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
