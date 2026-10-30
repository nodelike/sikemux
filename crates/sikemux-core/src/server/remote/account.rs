//! The account this host belongs to: signing in and out, and the live connection that tells it of devices, hosts and phones the account adds or revokes.

use super::*;

/// Signing in starts the live connection to the account and turns remote
/// access on. Signing out turns remote access off and sends `leave` on the
/// live connection, now or on the next one, so the account drops this host;
/// paired devices stay.
pub(crate) async fn set_owner(
    core: &Arc<Core>,
    owner: Option<String>,
    peer: &Peer,
) -> CoreResult<RemoteStatus> {
    if let Some(owner) = &owner {
        crate::accounts::check_user_id(owner).map_err(CoreError::from)?;
        core.remote.may_join(owner, peer)?;
        core.remote.bind(Some(owner.clone()))?;
    }
    let live = core.remote.inner.locked().accounts_api.is_some();
    let leaving = core.remote.change(|stored| {
        match owner {
            Some(owner) => {
                if stored.owner.as_ref() != Some(&owner) {
                    stored.account_event_id = 0;
                }
                stored.owner = Some(owner);
                stored.pending_leave = None;
                stored.removed = None;
                stored.enabled = true;
            }
            None => {
                stored.enabled = false;
                if let Some(previous) = stored.owner.take().filter(|_| live) {
                    stored.pending_leave = Some(previous);
                }
            }
        }
        Ok(stored.pending_leave.is_some())
    })?;
    if core.remote.owner().is_none() {
        unlink_all(core);
    }
    if core.remote.is_enabled() {
        if let Err(error) = listen(core).await {
            eprintln!("sikemux core: remote access did not start: {error}");
        }
    } else {
        stop(core).await;
    }
    ensure_live(core);
    if leaving {
        let confirmed = async {
            loop {
                let left = core.remote.left.notified();
                if core.remote.inner.locked().stored.pending_leave.is_none() {
                    return;
                }
                left.await;
            }
        };
        let _ = tokio::time::timeout(LEAVE_WAIT, confirmed).await;
    }
    Ok(announce(core))
}

/// Starts the live connection if the host wants one and has none, or wakes
/// the one it has to look again.
pub(crate) fn ensure_live(core: &Arc<Core>) {
    let mut inner = core.remote.inner.locked();
    if let Some(live) = &inner.live {
        let _ = live.changed.send(());
        return;
    }
    let Some(base) = inner.accounts_api.clone() else {
        return;
    };
    if inner.stored.want() == Want::Stop || inner.update_required.is_some() {
        return;
    }
    let (changed, watching) = watch::channel(());
    let app = LiveApp {
        platform: crate::server::host::PLATFORM,
        version: core.build.version.clone(),
    };
    let account = Arc::new(HostAccount(Arc::downgrade(core)));
    let outbox = inner.outbox.clone();
    let task = tokio::spawn(live::run(account, live::url(&base), app, outbox, watching));
    inner.live = Some(LiveTask { task, changed });
}

/// Takes the host that held the retired key off its account, trying again
/// until the account answers.
pub(super) fn leave_with_retired_key(core: &Arc<Core>) {
    let inner = core.remote.inner.locked();
    let (Some(key), Some(base)) = (inner.retired.clone(), inner.accounts_api.clone()) else {
        return;
    };
    let app = LiveApp {
        platform: crate::server::host::PLATFORM,
        version: core.build.version.clone(),
    };
    let account = Arc::new(RetiredHost {
        core: Arc::downgrade(core),
        key,
    });
    tokio::spawn(async move {
        let (_changed, watching) = watch::channel(());
        live::run(account, live::url(&base), app, Arc::default(), watching).await;
    });
}

pub(crate) fn stop_live(core: &Core) {
    let mut inner = core.remote.inner.locked();
    inner.link = None;
    if let Some(live) = inner.live.take() {
        live.task.abort();
    }
}

/// The host as its live connection sees it.
pub(super) struct HostAccount(Weak<Core>);

impl live::Account for HostAccount {
    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        self.0.upgrade()?.remote.sign_live(nonce)
    }

    fn want(&self) -> Want {
        let Some(core) = self.0.upgrade() else {
            return Want::Stop;
        };
        let want = {
            let mut inner = core.remote.inner.locked();
            let want = inner.stored.want();
            if want == Want::Stop {
                inner.live = None;
                inner.link = None;
            }
            want
        };
        if want == Want::Stop {
            announce(&core);
        }
        want
    }

    fn cursor(&self) -> i64 {
        self.0
            .upgrade()
            .map_or(0, |core| core.remote.inner.locked().stored.account_event_id)
    }

    fn apply(&self, events: &[AccountEvent]) {
        if let Some(core) = self.0.upgrade() {
            apply_events(&core, events);
            crate::server::stash::heard(&core, events);
        }
    }

    fn rewind(&self, latest: i64) {
        let Some(core) = self.0.upgrade() else {
            return;
        };
        if core.remote.inner.locked().stored.account_event_id <= latest {
            return;
        }
        let rewound = core.remote.change(|stored| {
            stored.account_event_id = 0;
            Ok(())
        });
        if let Err(error) = rewound {
            eprintln!("sikemux core: could not save the account's place: {error}");
        }
    }

    fn link(&self, link: Link) {
        let Some(core) = self.0.upgrade() else {
            return;
        };
        let changed = {
            let mut inner = core.remote.inner.locked();
            let changed = inner.link.map(|(current, _)| current) != Some(link);
            if changed {
                inner.link = Some((link, epoch_ms()));
            }
            changed
        };
        if changed {
            announce(&core);
        }
        if link == Link::Live {
            core.hosts.relink.now();
            crate::server::stash::catch_up(&core);
            crate::server::hooks::connect(&core);
        }
    }

    fn removed(&self, reason: Option<RevokeReason>) {
        if let Some(core) = self.0.upgrade() {
            let_go(&core, reason);
            announce(&core);
        }
    }

    fn pushed(&self, push: Push, result: PushResult) {
        if let Some(core) = self.0.upgrade() {
            if core.remote.note_pushed(&push, result) {
                announce(&core);
            }
        }
    }
}

/// The host as it was under its retired key, connecting only to leave.
pub(super) struct RetiredHost {
    core: Weak<Core>,
    key: SecretKey,
}

impl live::Account for RetiredHost {
    fn sign_live(&self, nonce: &str) -> Option<(String, String)> {
        sign_live(&self.key, nonce)
    }

    fn want(&self) -> Want {
        match self.core.upgrade() {
            Some(core) if core.remote.inner.locked().retired.is_some() => Want::Leave,
            _ => Want::Stop,
        }
    }

    fn cursor(&self) -> i64 {
        0
    }

    fn apply(&self, _: &[AccountEvent]) {}

    fn rewind(&self, _: i64) {}

    fn link(&self, _: Link) {}

    fn removed(&self, _: Option<RevokeReason>) {
        if let Some(core) = self.core.upgrade() {
            core.remote.forget_retired();
        }
    }

    fn pushed(&self, _: Push, _: PushResult) {}
}

pub(super) fn revoke_reason(reason: Option<RevokeReason>) -> &'static str {
    match reason {
        Some(RevokeReason::SignedOut) => "it signed out of your account",
        Some(RevokeReason::AccountDeleted) => "your account was deleted",
        _ => "it was removed from your account",
    }
}

/// Forgets each phone the account revoked and lets go of the account if it
/// let go of this host, then moves the cursor past `events`.
pub(super) fn apply_events(core: &Core, events: &[AccountEvent]) {
    let own = core.remote.core_id();
    let mut revoked = Vec::new();
    let mut released = None;
    for event in events {
        match event.r#type {
            AccountEventType::DeviceRevoked => match event.key.as_deref() {
                Some(key) if Some(key) == own.as_deref() => released = Some(event.reason),
                Some(key) => revoked.push((
                    key.to_owned(),
                    event.reason,
                    live::parse_rfc3339(&event.at).unwrap_or_else(epoch_ms),
                )),
                None => {}
            },
            AccountEventType::AccountDeleted => {
                released = Some(Some(RevokeReason::AccountDeleted));
            }
            AccountEventType::DeviceChanged if event.role == Some(DeviceRole::Host) => {
                core.hosts.relink.now();
            }
            AccountEventType::DeviceAdded => {
                if event.role == Some(DeviceRole::Host) {
                    core.hosts.relink.now();
                }
                // Events a host missed while offline come all at once; only a
                // device that joined lately is news.
                let lately = live::parse_rfc3339(&event.at)
                    .is_some_and(|at| epoch_ms().saturating_sub(at) < NEW_DEVICE_NEWS_MS);
                if let (true, Some(key), Some(core)) = (lately, &event.key, core.me.upgrade()) {
                    if Some(key) != own.as_ref() {
                        crate::server::hosts::account::tell_joined(&core, key.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let last = events.iter().map(|event| event.id).max().unwrap_or(0);
    let mut unlinked = Vec::new();
    let forgotten = core.remote.change(|stored| {
        let mut forgotten = Vec::new();
        for (key, reason, at) in &revoked {
            if stored.links.iter().any(|link| link.id == *key) {
                unlinked.push(key.clone());
            }
            if let Some(device) = stored.forget(key) {
                forgotten.push((device, *reason));
            }
            let latest = stored.revoked.entry(key.clone()).or_default();
            *latest = (*latest).max(*at);
        }
        let forgettable = epoch_ms().saturating_sub(join::REVOCATION_MEMORY_MS);
        stored.revoked.retain(|_, at| *at >= forgettable);
        stored.account_event_id = stored.account_event_id.max(last);
        Ok(forgotten)
    });
    let forgotten = forgotten.unwrap_or_else(|error| {
        eprintln!("sikemux core: could not apply what changed on the account: {error}");
        Vec::new()
    });
    {
        let mut inner = core.remote.inner.locked();
        inner.pending.retain(|pending| {
            !revoked
                .iter()
                .any(|(key, _, _)| *key == pending.device.device_id)
        });
    }
    for (device, reason) in &forgotten {
        core.close_device_clients(Some(&device.id));
        eprintln!(
            "sikemux core: forgot {} ({}): {}",
            if device.name.is_empty() {
                "a device"
            } else {
                &device.name
            },
            device.id.get(..8).unwrap_or(&device.id),
            revoke_reason(*reason)
        );
    }
    for host in unlinked {
        crate::server::hosts::unlinked(core, &host, "it was removed from your account");
    }
    if let Some(reason) = released {
        let_go(core, reason);
    }
    announce(core);
}

/// The account no longer has this host: forget it, keep why until the person
/// signs in again, and turn remote access off. Paired devices stay.
/// A host off its account is linked with none of the account's hosts; it
/// links with them again by itself once it signs in.
pub(super) fn unlink_all(core: &Core) {
    let links = core
        .remote
        .change(|stored| Ok(std::mem::take(&mut stored.links)));
    for link in links.unwrap_or_default() {
        crate::server::hosts::unlinked(core, &link.id, "this host left your account");
    }
}

pub(super) fn let_go(core: &Core, reason: Option<RevokeReason>) {
    let released = core.remote.change(|stored| {
        let asked = stored.pending_leave.is_some();
        stored.enabled = false;
        if stored.owner.take().is_some() {
            stored.removed = Some(Removal {
                reason,
                at: epoch_ms(),
            });
            eprintln!(
                "sikemux core: signed out of the account: {}",
                match reason {
                    Some(RevokeReason::AccountDeleted) => "it was deleted",
                    Some(RevokeReason::SignedOut) => "this host signed out elsewhere",
                    _ => "this host was removed from it",
                }
            );
        }
        stored.pending_leave = None;
        Ok(asked)
    });
    let released = released.and_then(|asked| {
        if asked {
            Ok(())
        } else {
            core.remote.bind(None)
        }
    });
    unlink_all(core);
    if let Err(error) = released {
        eprintln!("sikemux core: could not save leaving the account: {error}");
    }
    if let Some(endpoint) = halt(core) {
        tokio::spawn(async move { endpoint.close().await });
    }
    core.remote.left.notify_waiters();
}

pub(crate) fn revoke(core: &Core, id: &str) -> CoreResult<RemoteStatus> {
    let linked = core.remote.change(|stored| {
        let linked = stored.links.iter().any(|link| link.id == id);
        stored.forget(id);
        Ok(linked)
    })?;
    if linked {
        crate::server::hosts::unlink(core, id);
    }
    core.close_device_clients(Some(id));
    Ok(announce(core))
}

pub(super) fn account_link(inner: &Inner) -> Option<AccountLink> {
    if inner.stored.owner.is_some() {
        let (link, since) = inner.link?;
        let state = match link {
            Link::Connecting => AccountLinkState::Connecting,
            Link::Live => AccountLinkState::Live,
            Link::Offline => AccountLinkState::Offline,
        };
        return Some(AccountLink {
            state,
            reason: None,
            since,
        });
    }
    inner.stored.removed.map(|removal| AccountLink {
        state: AccountLinkState::Removed,
        reason: removal.reason,
        since: removal.at,
    })
}

impl Remote {
    pub(super) fn bind(&self, account: Option<String>) -> CoreResult<()> {
        let mut inner = self.inner.locked();
        inner.bound = account;
        self.save(&mut inner)
    }

    pub(crate) fn outbox(&self) -> Arc<live::Outbox> {
        self.inner.locked().outbox.clone()
    }
}
