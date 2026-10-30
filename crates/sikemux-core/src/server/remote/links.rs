//! The other hosts on the account this one is linked with.

use super::*;

impl Remote {
    /// The hosts asking to link with this one.
    pub(crate) fn pending_links(&self) -> Vec<PendingDevice> {
        self.inner
            .locked()
            .pending
            .iter()
            .filter(|pending| pending.host.is_some())
            .map(|pending| pending.device.clone())
            .collect()
    }

    pub(crate) fn links(&self) -> Vec<LinkedHost> {
        let inner = self.inner.locked();
        if !inner.stored.enabled {
            return Vec::new();
        }
        inner.stored.links.clone()
    }

    /// The linked host with this key, while remote access is on.
    pub(crate) fn link(&self, id: &str) -> Option<LinkedHost> {
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
    pub(crate) fn name_links(&self, names: &[(String, String)]) -> CoreResult<bool> {
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
    pub(crate) fn forget_links_off(&self, account: &[String]) -> CoreResult<Vec<String>> {
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
            Ok(gone)
        })
    }

    /// Keeps a host this one is now linked with, replacing what it knew of it.
    pub(crate) fn add_link(&self, link: LinkedHost) -> CoreResult<()> {
        self.change(|stored| {
            stored.forget(&link.id);
            stored.links.push(link);
            Ok(())
        })
    }

    /// Forgets a linked host, and whether it was one.
    pub(crate) fn forget_link(&self, id: &str) -> bool {
        let forgotten = self.change(|stored| {
            let before = stored.links.len();
            stored.links.retain(|link| link.id != id);
            Ok(stored.links.len() != before)
        });
        forgotten.unwrap_or_else(|error| {
            eprintln!("sikemux core: could not save forgetting a host: {error}");
            false
        })
    }

    /// Notes that a linked host was reached, and where.
    pub(crate) fn link_seen(&self, id: &str, addresses: Option<Vec<String>>) {
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
