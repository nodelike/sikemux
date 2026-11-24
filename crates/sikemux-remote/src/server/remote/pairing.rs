//! Letting devices in: join tickets, the person's answer to each new device, and what each may do.

use super::*;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::notify::Signal;
use crate::server::remote_state::RemoteParts;
pub use sikemux_calls::remote::ApprovalSetting;
pub use sikemux_calls::remote::{DeviceChange, PairingAnswer, RemotePairing};

pub fn set_access(core: &Core, id: &str, access: DeviceAccess) -> CoreResult<RemoteStatus> {
    core.remote().change(|stored| {
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
pub fn rename_paired(core: &Core, id: &str, name: &str) -> CoreResult<()> {
    let renamed = core.remote().change(|stored| {
        let device = stored.devices.iter_mut().find(|device| device.id == id);
        Ok(device.map(|device| device.name = name.to_owned()).is_some())
    })?;
    if renamed {
        announce(core);
    }
    Ok(())
}

pub fn pairing(core: &Core) -> RemotePairing {
    let status = core.remote().status();
    RemotePairing {
        pending: status.pending,
        devices: status.devices,
        links: (core.remote_above().linked_hosts)(core),
        pending_links: core.remote().pending_links(),
    }
}

pub fn answer_pairing(core: &Core, answer: PairingAnswer) -> CoreResult<RemotePairing> {
    core.remote()
        .answer(&answer.id, answer.allow.then_some(answer.access))?;
    announce(core);
    Ok(pairing(core))
}

/// What the person here said to a device or host asking to be let in.
pub enum Answer {
    Allowed(DeviceAccess),
    Denied,
    /// No answer in time, or the account took it off meanwhile.
    None,
    /// The same host asked again, and that request has the person's answer.
    TakenOver,
}

/// Asks the person here to let `device` in, as a card and a notification on every device acting
/// as them.
pub fn ask_person(core: &Core, device: PendingDevice, account: String, asking: Asking) -> Asked {
    let asked = core.remote().ask(device, account, asking);
    if asked.new {
        announce(core);
        core.notify().send(Signal::Pairing(asked.request.clone()));
    }
    asked
}

/// Waits for what the person says to `asked`.
pub async fn answer_to(core: &Core, asked: Asked) -> Answer {
    let Asked {
        request,
        joining,
        answered,
        ..
    } = asked;
    let left = Duration::from_millis(request.expires_at.saturating_sub(epoch_ms()));
    let answer = match tokio::time::timeout(left, answered).await {
        Ok(Ok(Some(access))) => Answer::Allowed(access),
        Ok(Ok(None)) => Answer::Denied,
        Ok(Err(_)) if core.remote().is_pending(&request.id) => return Answer::TakenOver,
        _ => Answer::None,
    };
    core.remote().forget_pending(&request.id);
    announce(core);
    core.notify()
        .send(Signal::PairingSettled { id: request.id });
    if joining == Joining::Host {
        look_again(core);
    }
    answer
}

/// Links with the account's other hosts and seals the vaults' keys for them again, after a
/// change to which of them the person approved.
fn look_again(core: &Core) {
    (core.remote_above().hosts_relink)(core);
    core.vault().wake();
    core.vault().orgs().wake();
}

/// Turns the lock of ADR 0018 on or off: whether each new device and host
/// waits for the person rather than being let in on its account's word.
pub fn set_require_approval(core: &Core, setting: ApprovalSetting) -> CoreResult<RemoteStatus> {
    core.remote().change(|stored| {
        stored.require_approval = setting.require_approval;
        Ok(())
    })?;
    core.remote().inner.locked().asked_about.clear();
    core.remote().settle_listed();
    look_again(core);
    Ok(announce(core))
}

pub fn change_device(core: &Core, change: DeviceChange) -> CoreResult<RemotePairing> {
    match change.access {
        Some(access) => set_access(core, &change.id, access)?,
        None => revoke(core, &change.id)?,
    };
    Ok(pairing(core))
}

/// Tells the app what changed, and answers with the same status.
pub fn announce(core: &Core) -> RemoteStatus {
    if let Some(core) = core.arc() {
        tokio::spawn(async move { crate::server::account::show(&core).await });
    }
    core.changed(What::Read(Topic::Hosts));
    let status = core.remote().status();
    core.changed(What::Event(&Event::Remote {
        status: status.clone(),
    }));
    status
}

impl Remote {
    pub fn join_keys(&self) -> TrustedKeys {
        #[cfg(any(test, feature = "testing"))]
        if let Some(keys) = self.inner.locked().join_keys.clone() {
            return keys;
        }
        TrustedKeys::for_this_build()
    }

    pub fn add_device(&self, mut device: DeviceInfo, account: Option<String>) -> CoreResult<()> {
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
    pub fn forget_phones_off(
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
    pub fn requires_approval(&self) -> bool {
        self.inner.locked().stored.require_approval
    }

    /// Whether the key `id` may join without the person being asked. One they have not approved
    /// waits for them when they asked to approve each, or removed this one before.
    pub fn admits(&self, id: &str, joining: Joining) -> bool {
        self.inner.locked().stored.admits(id, joining)
    }

    /// A host with this key, signed in to `owner`, with these phones paired.
    #[cfg(any(test, feature = "testing"))]
    pub fn stand_in(&self, secret: SecretKey, owner: Option<&str>, devices: Vec<DeviceInfo>) {
        let mut inner = self.inner.locked();
        inner.secret = Some(secret);
        inner.stored.owner = owner.map(str::to_owned);
        inner.stored.devices = devices;
    }
}
