//! A remote file that failed its check: kept aside with remote access off,
//! until the person trusts what it named again or lets it go.

use super::*;
use crate::accounts::protocol::DeviceList;
use crate::server::account::api::as_host;
use crate::server::remote_state::RemoteParts;
use reqwest::Method;

/// The remote file set aside beside `path`, and when it was, if there is one.
pub(super) fn read_held(path: &Path) -> Option<(Stored, u64)> {
    let held = held_path(path);
    let at = std::fs::metadata(&held)
        .and_then(|meta| meta.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    match read_stored(&held) {
        Ok(stored) => Some((stored, at)),
        Err(error) => {
            eprintln!(
                "sikemux core: {} could not be read: {error}",
                held.display()
            );
            None
        }
    }
}

/// Brings back the devices and hosts the set-aside file named that the
/// account still lists, as the person chose, beside whatever is paired now.
/// Nothing else in that file is trusted: not its account, shares, settings or
/// removals, and not a device or host removed or revoked here since.
pub(crate) async fn trust_held(core: &Arc<Core>) -> CoreResult<RemoteStatus> {
    let owner = core.remote().owner().ok_or_else(|| {
        CoreError::from("sign in first, so the account can say which devices are still yours")
    })?;
    let Some((held, _)) = core
        .remote()
        .inner
        .locked()
        .held
        .as_ref()
        .map(|(held, at)| ((held.devices.clone(), held.links.clone()), *at))
    else {
        return Err("nothing waits to be trusted again".into());
    };
    let (devices, links) = held;
    let phones: DeviceList = as_host(core, Method::GET, "/v1/devices?role=client", None).await?;
    let hosts: DeviceList = as_host(core, Method::GET, "/v1/devices?role=host", None).await?;
    let own = core.remote().core_id();
    core.remote().change(|stored| {
        if stored.owner.as_ref() != Some(&owner) {
            return Err("the account changed meanwhile; try again".into());
        }
        let kept = |stored: &Stored, id: &str| {
            !stored.removed_here.contains_key(id)
                && !stored.revoked.contains_key(id)
                && !stored.devices.iter().any(|device| device.id == id)
                && !stored.links.iter().any(|link| link.id == id)
        };
        for device in devices {
            if phones.devices.iter().any(|phone| phone.key == device.id) && kept(stored, &device.id)
            {
                stored
                    .joined_through
                    .insert(device.id.clone(), owner.clone());
                stored.devices.push(DeviceInfo {
                    last_seen: None,
                    ..device
                });
            }
        }
        for link in links {
            let Some(host) = hosts.devices.iter().find(|host| host.key == link.id) else {
                continue;
            };
            if Some(&link.id) != own.as_ref() && kept(stored, &link.id) {
                stored.links.push(StoredLink {
                    name: host.name.clone(),
                    last_seen: None,
                    addresses: Vec::new(),
                    ..link
                });
            }
        }
        stored.enabled = true;
        Ok(())
    })?;
    core.remote().inner.locked().held = None;
    forget_held_file(core);
    if let Err(error) = listen(core).await {
        eprintln!("sikemux core: remote access did not start: {error}");
    }
    ensure_live(core);
    (core.remote_above().hosts_relink)(core);
    Ok(announce(core))
}

/// Lets the set-aside file go: the host starts over with no devices or hosts.
pub(crate) fn discard_held(core: &Core) -> CoreResult<RemoteStatus> {
    if core.remote().inner.locked().held.take().is_none() {
        return Err("nothing waits to be trusted again".into());
    }
    forget_held_file(core);
    Ok(announce(core))
}

fn forget_held_file(core: &Core) {
    let Some(path) = core.remote().inner.locked().path.clone() else {
        return;
    };
    if let Err(error) = std::fs::remove_file(held_path(&path)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("sikemux core: could not remove the set-aside remote file: {error}");
        }
    }
}
