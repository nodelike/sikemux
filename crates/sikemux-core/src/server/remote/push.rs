//! Notifications for paired phones: what each asked to hear, and what became of the last one sent.

use super::*;

/// A phone to notify, with its key and what it asked for.
#[derive(Clone, Debug)]
pub(crate) struct NotifyTarget {
    pub device_id: String,
    pub key: NotificationKey,
    pub prefs: NotifyPrefs,
}

pub(super) const MAX_MUTES: usize = 256;

pub(super) const MAX_AGENT_ID: usize = 200;

pub(super) fn notifications(inner: &Inner) -> Vec<PhoneNotifications> {
    let signed_out = inner.stored.owner.is_none();
    inner
        .stored
        .notifications
        .iter()
        .filter(|(id, _)| inner.stored.devices.iter().any(|device| &device.id == *id))
        .map(|(id, notify)| {
            let pushed = inner.pushed.get(id).copied();
            let (state, since) = if notify.prefs.when == NotifyWhen::Off {
                (NotificationState::Off, notify.since)
            } else if signed_out {
                (NotificationState::SignedOut, notify.since)
            } else {
                pushed.unwrap_or((NotificationState::On, notify.since))
            };
            PhoneNotifications {
                device_id: id.clone(),
                state,
                since,
            }
        })
        .collect()
}

impl Remote {
    /// Keeps a paired phone's notification key and preferences, replacing
    /// what it sent before.
    pub(crate) fn set_notifications(
        &self,
        id: &str,
        key_id: u32,
        key: &str,
        prefs: NotifyPrefs,
    ) -> CoreResult<()> {
        NotificationKey::from_hex(key_id, key)
            .ok_or_else(|| CoreError::from("the notification key is 32 bytes in hex"))?;
        if prefs.muted.len() > MAX_MUTES
            || prefs
                .muted
                .iter()
                .any(|mute| mute.agent_id.len() > MAX_AGENT_ID)
        {
            return Err("more muted agents than the host keeps".into());
        }
        self.change(|stored| {
            if !stored.devices.iter().any(|device| device.id == id) {
                return Err(CoreError::from(crate::server::access::UNPAIRED));
            }
            let since = match stored.notifications.get(id) {
                Some(known) if known.key_id == key_id && known.key == key => known.since,
                _ => epoch_ms(),
            };
            stored.notifications.insert(
                id.to_owned(),
                PhoneNotify {
                    key_id,
                    key: key.to_ascii_lowercase(),
                    prefs,
                    since,
                },
            );
            Ok(())
        })?;
        let mut inner = self.inner.locked();
        inner.refused.remove(id);
        Ok(())
    }

    pub(crate) fn clear_notifications(&self, id: &str) -> CoreResult<()> {
        self.change(|stored| {
            stored.notifications.remove(id);
            Ok(())
        })?;
        let mut inner = self.inner.locked();
        inner.pushed.remove(id);
        inner.refused.remove(id);
        Ok(())
    }

    /// This host's key and the phones it may notify now: none while it is
    /// signed out, since notifications go through the account.
    pub(crate) fn notify_targets(&self) -> Option<(String, Vec<NotifyTarget>)> {
        let inner = self.inner.locked();
        if inner.stored.owner.is_none() || inner.update_required.is_some() {
            return None;
        }
        let host = inner.secret.as_ref()?.public().to_string();
        let targets = inner
            .stored
            .notifications
            .iter()
            .filter(|(id, _)| !inner.refused.contains(*id))
            .filter(|(id, _)| inner.stored.devices.iter().any(|device| &device.id == *id))
            .filter_map(|(id, notify)| {
                Some(NotifyTarget {
                    device_id: id.clone(),
                    key: NotificationKey::from_hex(notify.key_id, &notify.key)?,
                    prefs: notify.prefs.clone(),
                })
            })
            .collect();
        Some((host, targets))
    }

    pub(crate) fn note_pushed(&self, push: &Push, result: PushResult) -> bool {
        let state = match result {
            PushResult::Sent => NotificationState::On,
            PushResult::NoToken => NotificationState::PhoneOff,
            PushResult::NotAllowed => NotificationState::OtherAccount,
            PushResult::Failed | PushResult::NotSetUp => NotificationState::NotReaching,
            PushResult::Throttled | PushResult::Expired | PushResult::Unknown => return false,
        };
        let mut inner = self.inner.locked();
        if !inner.stored.notifications.contains_key(&push.to) {
            return false;
        }
        if state == NotificationState::OtherAccount {
            inner.refused.insert(push.to.clone());
        }
        let changed = inner.pushed.get(&push.to).map(|(known, _)| *known) != Some(state);
        if changed {
            inner.pushed.insert(push.to.clone(), (state, epoch_ms()));
        }
        changed
    }
}
