//! A remote file that failed its check: kept aside with remote access off,
//! until the person trusts what it named again or lets it go.

use super::*;

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

/// Trusts the devices, hosts and account the set-aside file named again, as
/// the person chose, and turns remote access back on if it was on.
pub(crate) async fn trust_held(core: &Arc<Core>) -> CoreResult<RemoteStatus> {
    let enabled = {
        let mut inner = core.remote.inner.locked();
        let (mut held, _) = inner
            .held
            .take()
            .ok_or_else(|| CoreError::from("nothing waits to be trusted again"))?;
        held.secret_key = None;
        if held.owner.is_some() && inner.bound.is_some() && held.owner != inner.bound {
            held.owner = None;
            held.enabled = false;
        }
        held.network = inner.stored.network.take();
        inner.stored = held;
        if let Err(error) = core.remote.save(&mut inner) {
            let restored = std::mem::take(&mut inner.stored);
            inner.held = Some((restored, epoch_ms()));
            return Err(error);
        }
        inner.stored.enabled
    };
    forget_held_file(core);
    if enabled {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    }
    ensure_live(core);
    Ok(announce(core))
}

/// Lets the set-aside file go: the host starts over with no devices or hosts.
pub(crate) fn discard_held(core: &Core) -> CoreResult<RemoteStatus> {
    if core.remote.inner.locked().held.take().is_none() {
        return Err("nothing waits to be trusted again".into());
    }
    forget_held_file(core);
    Ok(announce(core))
}

fn forget_held_file(core: &Core) {
    let Some(path) = core.remote.inner.locked().path.clone() else {
        return;
    };
    if let Err(error) = std::fs::remove_file(held_path(&path)) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("sikemux core: could not remove the set-aside remote file: {error}");
        }
    }
}
