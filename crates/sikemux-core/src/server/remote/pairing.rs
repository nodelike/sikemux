//! Letting devices in: join tickets, the person's answer to each new device, and what each may do.

use super::*;
use crate::server::changed::What;

pub(crate) fn set_access(core: &Core, id: &str, access: DeviceAccess) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        let device = stored
            .devices
            .iter_mut()
            .find(|device| device.id == id)
            .ok_or_else(|| CoreError::from("no paired device has that id"))?;
        device.access = access;
        Ok(())
    })?;
    Ok(announce(core))
}

/// Gives the device `id` paired here the name the account now has for it.
pub(crate) fn rename_paired(core: &Core, id: &str, name: &str) -> CoreResult<()> {
    let renamed = core.remote.change(|stored| {
        let device = stored.devices.iter_mut().find(|device| device.id == id);
        Ok(device.map(|device| device.name = name.to_owned()).is_some())
    })?;
    if renamed {
        announce(core);
    }
    Ok(())
}

/// Who waits to join this host and who is paired, and the hosts linked with
/// it or asking to be, for the owner's devices.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePairing {
    pub pending: Vec<PendingDevice>,
    pub devices: Vec<DeviceInfo>,
    pub links: Vec<crate::server::hosts::LinkedHost>,
    pub pending_links: Vec<PendingDevice>,
}

pub(crate) fn pairing(core: &Core) -> RemotePairing {
    let status = core.remote.status();
    RemotePairing {
        pending: status.pending,
        devices: status.devices,
        links: crate::server::hosts::views(core),
        pending_links: core.remote.pending_links(),
    }
}

#[derive(Deserialize)]
pub struct PairingAnswer {
    id: String,
    allow: bool,
    access: DeviceAccess,
}

pub(crate) fn answer_pairing(core: &Core, answer: PairingAnswer) -> CoreResult<RemotePairing> {
    core.remote
        .answer(&answer.id, answer.allow.then_some(answer.access))?;
    announce(core);
    Ok(pairing(core))
}

#[derive(Deserialize)]
pub struct DeviceChange {
    id: String,
    /// None unpairs the device.
    #[serde(default)]
    access: Option<DeviceAccess>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSetting {
    pub require_approval: bool,
}

/// Turns the lock of ADR 0018 on or off: whether each new device and host
/// waits for the person rather than being let in on its account's word.
pub(crate) fn set_require_approval(
    core: &Core,
    setting: ApprovalSetting,
) -> CoreResult<RemoteStatus> {
    core.remote.change(|stored| {
        stored.require_approval = setting.require_approval;
        Ok(())
    })?;
    Ok(announce(core))
}

pub(crate) fn change_device(core: &Core, change: DeviceChange) -> CoreResult<RemotePairing> {
    match change.access {
        Some(access) => set_access(core, &change.id, access)?,
        None => revoke(core, &change.id)?,
    };
    Ok(pairing(core))
}

/// Tells the app what changed, and answers with the same status.
pub(crate) fn announce(core: &Core) -> RemoteStatus {
    if let Some(core) = core.arc() {
        tokio::spawn(async move { crate::server::account::show(&core).await });
    }
    core.changed(What::Read(Topic::Hosts));
    let status = core.remote.status();
    core.changed(What::Event(&Event::Remote {
        status: status.clone(),
    }));
    status
}

impl Remote {
    pub(super) fn join_keys(&self) -> TrustedKeys {
        #[cfg(test)]
        if let Some(keys) = self.inner.locked().join_keys.clone() {
            return keys;
        }
        TrustedKeys::for_this_build()
    }

    pub(super) fn add_device(
        &self,
        mut device: DeviceInfo,
        account: Option<String>,
    ) -> CoreResult<()> {
        self.change(|stored| {
            stored.forget(&device.id);
            if stored.removed_here.remove(&device.id) == Some(DeviceAccess::Watch) {
                device.access = DeviceAccess::Watch;
            }
            if let Some(account) = account {
                stored.joined_through.insert(device.id.clone(), account);
            }
            stored.devices.push(device);
            Ok(())
        })
    }

    /// Forgets every phone that joined through `owner` before `since` and
    /// that `account`, the phones `owner`'s account listed from then on, no
    /// longer holds. Nothing changes if the host moved to another account
    /// meanwhile. A ticket issued before now is refused for each.
    pub(crate) fn forget_phones_off(
        &self,
        owner: &str,
        since: u64,
        account: &[String],
    ) -> CoreResult<Vec<DeviceInfo>> {
        self.change(|stored| {
            if stored.owner.as_deref() != Some(owner) {
                return Ok(Vec::new());
            }
            let gone: Vec<String> = stored
                .devices
                .iter()
                .filter(|device| device.paired_at < since)
                .filter(|device| {
                    stored.joined_through.get(&device.id).map(String::as_str) == Some(owner)
                })
                .filter(|device| !account.contains(&device.id))
                .map(|device| device.id.clone())
                .collect();
            let now = epoch_ms();
            let mut forgotten = Vec::new();
            for id in gone {
                forgotten.extend(stored.forget(&id));
                stored.revoked.insert(id, now);
            }
            Ok(forgotten)
        })
    }

    /// Whether a device or host with a good ticket waits for the person.
    pub(crate) fn requires_approval(&self) -> bool {
        self.inner.locked().stored.require_approval
    }

    /// Whether the device `id` with a good ticket waits for the person: it
    /// does when they asked to approve each one, or removed this one before.
    pub(crate) fn requires_approval_of(&self, id: &str) -> bool {
        let inner = self.inner.locked();
        inner.stored.require_approval || inner.stored.removed_here.contains_key(id)
    }

    /// A host with this key, signed in to `owner`, with these phones paired.
    #[cfg(test)]
    pub(crate) fn stand_in(
        &self,
        secret: SecretKey,
        owner: Option<&str>,
        devices: Vec<DeviceInfo>,
    ) {
        let mut inner = self.inner.locked();
        inner.secret = Some(secret);
        inner.stored.owner = owner.map(str::to_owned);
        inner.stored.devices = devices;
    }
}
