//! The vault as Settings → Connectors shows it, and what the person does there: save the
//! recovery key, open the vault with it, and keep a sign-in on this host, out of the vault.

use crate::server::remote_state::RemoteParts;
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::vault::{KeepNewParams, KeepParams, RecoverParams};
pub use sikemux_calls::vault::{ShownSignIn, ShownStatus, ShownVault};
use std::sync::Arc;

use sikemux_process::sync::Locked;

use super::super::{connection::blocking, Core, CoreError, CoreResult};
use super::{local, sync, State, Status};
use sikemux_calls::hosts::Reach;

/// What the account calls a host, and whether this host reaches it now, or the start of its
/// key when the account does not name it.
pub fn named(core: &Core, state: &State, key: &str) -> (String, bool) {
    if state.me.as_deref() == Some(key) {
        return (core.notify().host_name(), true);
    }
    let linked = (core.remote_above().host_rows)(core)
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

pub fn vault(core: &Core) -> ShownVault {
    let state = core.vault().state.locked();
    let (status, failed) = match &state.status {
        Status::Off => (ShownStatus::Off, None),
        Status::Waiting => (ShownStatus::Waiting, None),
        Status::Open => (ShownStatus::Open, None),
        Status::Failed(why) => (ShownStatus::Failed, Some(why.clone())),
    };
    let sign_ins = state
        .entries
        .values()
        .filter(|entry| {
            !state
                .marks
                .kept(&entry.body.plugin, &entry.body.sign_in.identity)
        })
        .map(|entry| ShownSignIn {
            plugin: entry.body.plugin.clone(),
            identity: entry.body.sign_in.identity.clone(),
            label: entry.body.sign_in.label.clone(),
        })
        .collect();
    ShownVault {
        status,
        failed,
        holders: state.holders.len(),
        recovery: state.recovery_text.clone(),
        sign_ins,
        kept: state
            .here
            .iter()
            .filter(|held| state.marks.kept(&held.plugin, &held.identity))
            .map(|held| ShownSignIn {
                plugin: held.plugin.clone(),
                identity: held.identity.clone(),
                label: held.label.clone(),
            })
            .collect(),
        keep_new: state.marks.keep_new,
    }
}

/// Keeps a sign-in on this host and out of the vault, or puts it back in. The next round
/// takes it out of the vault, or puts it in as a new sign-in for every host.
pub async fn keep(core: &Arc<Core>, params: KeepParams) -> CoreResult<()> {
    let label = {
        let state = core.vault().state.locked();
        state
            .here
            .iter()
            .find(|held| held.plugin == params.plugin && held.identity == params.identity)
            .map(|held| held.label.clone())
    };
    let Some(label) = label else {
        return Err("this host does not hold that sign-in".into());
    };
    change_marks(core, move |marks| {
        marks.set(local::Mark {
            plugin: params.plugin,
            identity: params.identity,
            label,
            kept: params.kept,
        });
    })
    .await
}

/// Whether sign-ins first seen on this host from now on stay here.
pub async fn keep_new(core: &Arc<Core>, params: KeepNewParams) -> CoreResult<()> {
    change_marks(core, move |marks| marks.keep_new = params.on).await
}

async fn change_marks(
    core: &Arc<Core>,
    change: impl FnOnce(&mut local::Marks) + Send + 'static,
) -> CoreResult<()> {
    let data_dir = core.data_dir().clone();
    let marks =
        blocking(move || local::change_marks(data_dir.as_deref(), change).map_err(CoreError::from))
            .await?;
    {
        let mut state = core.vault().state.locked();
        state.marks = marks;
        state.dirty = true;
    }
    core.vault().wake();
    super::announce(core);
    Ok(())
}

pub async fn recovery_saved(core: &Arc<Core>) -> CoreResult<()> {
    let account = core.vault().state.locked().account.clone();
    if let Some(account) = account {
        blocking(move || local::forget_recovery(&account).map_err(CoreError::from)).await?;
    }
    core.vault().state.locked().recovery_text = None;
    super::announce(core);
    super::inbox_changed(core);
    Ok(())
}

pub async fn recover(core: &Arc<Core>, params: RecoverParams) -> CoreResult<()> {
    sync::recover(core, &params.key).await
}
