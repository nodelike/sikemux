//! The other hosts on the account this one is linked with.

use super::*;

impl Remote {
    pub fn pending_links(&self) -> Vec<PendingDevice> {
        self.inner
            .locked()
            .pending
            .iter()
            .filter(|pending| pending.asking.joining() == Joining::Host)
            .map(|pending| pending.device.clone())
            .collect()
    }

    pub fn links(&self) -> Vec<StoredLink> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return Vec::new();
        }
        inner.stored.links.clone()
    }

    /// The linked host with this key, while remote access is on.
    pub fn link(&self, id: &str) -> Option<StoredLink> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return None;
        }
        inner
            .stored
            .links
            .iter()
            .find(|link| link.id == id)
            .cloned()
    }

    /// Calls the linked hosts what the account calls them, answering whether
    /// any name changed.
    pub fn name_links(&self, names: &[(String, String)]) -> CoreResult<bool> {
        self.change(|stored| {
            let mut changed = false;
            for link in &mut stored.links {
                if let Some((_, name)) = names.iter().find(|(id, _)| *id == link.id) {
                    if !name.is_empty() && link.name != *name {
                        link.name.clone_from(name);
                        changed = true;
                    }
                }
            }
            Ok(changed)
        })
    }

    /// Forgets every linked host the account no longer has, answering which.
    pub fn forget_links_off(&self, account: &[String]) -> CoreResult<Vec<String>> {
        self.change(|stored| {
            let gone: Vec<String> = stored
                .links
                .iter()
                .map(|link| link.id.clone())
                .filter(|id| !account.contains(id))
                .collect();
            for id in &gone {
                stored.forget(id);
            }
            stored.approved_hosts.retain(|host| account.contains(host));
            Ok(gone)
        })
    }

    /// Keeps a host this one is now linked with, unless it has to wait for the person first.
    pub fn add_link(&self, link: StoredLink, how: AdmittedBy) -> CoreResult<()> {
        self.change(|stored| stored.link(link, how))?;
        self.settle_listed();
        Ok(())
    }

    /// Takes back each question about a host the account lists that no longer needs an answer.
    pub(super) fn settle_listed(&self) {
        let mut inner = self.inner.locked();
        let Inner {
            pending, stored, ..
        } = &mut *inner;
        pending.retain(|asked| {
            !matches!(asked.asking, Asking::Listed)
                || !stored.admits(&asked.device.device_id, Joining::Host)
        });
    }

    /// Notes that the person here let the host with this key in.
    pub fn approve_host(&self, id: &str) -> CoreResult<()> {
        self.change(|stored| {
            stored.approve_host(id);
            Ok(())
        })?;
        self.settle_listed();
        Ok(())
    }

    /// Whether a vault's key is sealed for the host with this key: this host's own, any the
    /// person here approved, and while they did not ask to approve each, any the account lists.
    pub fn vault_open_to(&self, id: &str) -> bool {
        let inner = self.inner.locked();
        let own = inner.secret.as_ref().map(SecretKey::public);
        own.is_some_and(|own| own.to_string() == id)
            || !inner.stored.require_approval
            || inner.stored.approved(id, Joining::Host)
    }

    /// When, from `now` on, the person may be asked about a host the account lists: once a day for
    /// each. None while they are being asked about it, or about as many as they are at once.
    pub fn may_ask_about(&self, id: &str, now: u64) -> Option<u64> {
        const AGAIN_MS: u64 = 24 * 60 * 60 * 1000;
        const AT_ONCE: usize = 4;
        let inner = self.inner.locked();
        let mut listed = 0;
        for pending in &inner.pending {
            if pending.asking.joining() == Joining::Host && pending.device.device_id == id {
                return None;
            }
            listed += usize::from(matches!(pending.asking, Asking::Listed));
        }
        match inner.asked_about.get(id) {
            Some(at) if at + AGAIN_MS > now => Some(at + AGAIN_MS),
            _ => (listed < AT_ONCE).then_some(now),
        }
    }

    /// The hosts on the account whose person turned down linking with this
    /// one.
    pub fn declined_links(&self) -> BTreeSet<String> {
        self.inner.locked().stored.declined_links.clone()
    }

    /// Remembers that the person at `id` turned down linking, keeping only
    /// the hosts still on the account, `account`.
    pub fn decline_link(&self, id: &str, account: &[String]) -> CoreResult<()> {
        self.change(|stored| {
            stored.declined_links.insert(id.to_owned());
            stored.declined_links.retain(|host| account.contains(host));
            Ok(())
        })
    }

    /// Whether the person removed the host `id` from this one, which then
    /// links again only when the person says so.
    pub fn removed_here(&self, id: &str) -> bool {
        self.inner.locked().stored.removed_here.contains_key(id)
    }

    /// When the host `id` may next be asked to link, after failing lately.
    pub fn relink_due(&self, id: &str) -> Option<u64> {
        let inner = self.inner.locked();
        inner.relink_failures.get(id).map(|(_, due)| *due)
    }

    /// Notes that linking with `id` failed, and answers when to ask again,
    /// waiting longer with each failure in a row, up to five minutes.
    pub fn relink_failed(&self, id: &str) -> u64 {
        const WAIT: crate::backoff::Backoff = crate::backoff::Backoff {
            first: Duration::from_secs(30),
            cap: Duration::from_secs(5 * 60),
        };
        let mut inner = self.inner.locked();
        let (failures, due) = inner.relink_failures.entry(id.to_owned()).or_default();
        *failures = failures.saturating_add(1);
        *due = epoch_ms() + WAIT.after(*failures).as_millis() as u64;
        *due
    }

    pub fn relink_succeeded(&self, id: &str) {
        self.inner.locked().relink_failures.remove(id);
    }

    /// Forgets a linked host, and whether it was one.
    pub fn forget_link(&self, id: &str) -> bool {
        let forgotten = self.change(|stored| {
            let before = stored.links.len();
            stored.approved_hosts.remove(id);
            stored.links.retain(|link| link.id != id);
            Ok(stored.links.len() != before)
        });
        forgotten.unwrap_or_else(|error| {
            eprintln!("sikemux core: could not save forgetting a host: {error}");
            false
        })
    }

    /// Notes that a linked host was reached, and where.
    pub fn link_seen(&self, id: &str, addresses: Option<Vec<String>>) {
        let mut inner = self.inner.locked();
        let Some(link) = inner.stored.links.iter_mut().find(|link| link.id == id) else {
            return;
        };
        link.last_seen = Some(epoch_ms());
        if let Some(addresses) = addresses.filter(|addresses| !addresses.is_empty()) {
            link.addresses = addresses;
        }
        if let Err(error) = self.save(&mut inner) {
            eprintln!("sikemux core: could not save when a host was last seen: {error}");
        }
    }
}
