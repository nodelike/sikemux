//! Letting devices in: join tickets, the person's answer to each new device, and what each may do.

use super::*;

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

/// Who waits to join this host and who is paired, and the hosts linked with
/// it or asking to be, for the owner's devices.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pending: Vec<PendingDevice>,
    devices: Vec<DeviceInfo>,
    links: Vec<crate::server::hosts::LinkView>,
    pending_links: Vec<PendingDevice>,
}

pub(crate) fn pairing(core: &Core) -> Pairing {
    let status = core.remote.status();
    Pairing {
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

pub(crate) fn answer_pairing(core: &Core, answer: PairingAnswer) -> CoreResult<Pairing> {
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

pub(crate) fn change_device(core: &Core, change: DeviceChange) -> CoreResult<Pairing> {
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
    core.changes.changed(Topic::Hosts, None, None);
    let status = core.remote.status();
    core.broadcast_local(&Event::Remote {
        status: status.clone(),
    });
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

    pub(super) fn add_device(&self, device: DeviceInfo, account: Option<String>) -> CoreResult<()> {
        self.change(|stored| {
            stored.forget(&device.id);
            if let Some(account) = account {
                stored.joined_through.insert(device.id.clone(), account);
            }
            stored.devices.push(device);
            Ok(())
        })
    }

    /// Whether a device or host with a good ticket waits for the person.
    pub(crate) fn requires_approval(&self) -> bool {
        self.inner.locked().stored.require_approval
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
