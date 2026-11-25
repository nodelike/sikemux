//! Who this host let in, and telling the person and the linked hosts of it.

use super::*;
use crate::server::changed::{ChangedCore, What};
use crate::server::join::{clean, NAME_LIMIT, PLATFORM_LIMIT};
use crate::server::remote_state::RemoteParts;
pub use sikemux_calls::remote::{Admission, AdmittedBy};

const KEPT: usize = 200;
const TOLD_KEPT: usize = 1000;

/// Someone let in, and whether the person has not heard of them yet.
pub struct News {
    admission: Admission,
    tell: bool,
}

impl Stored {
    /// Brings the records in line with who is on this host: one that came gets a record, and
    /// one that went has its record marked as left. Answers those that came.
    pub(super) fn record_admissions(&mut self, now: u64) -> Vec<News> {
        let devices = self.devices.iter().map(|device| Admission {
            key: device.id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            host: false,
            via: device.via.clone(),
            at: device.paired_at,
            how: AdmittedBy::Unsaid,
            left_at: None,
            told_by: None,
            told_with: Vec::new(),
        });
        let links = self.links.iter().map(|link| Admission {
            key: link.id.clone(),
            name: link.name.clone(),
            platform: link.platform.clone(),
            host: true,
            via: None,
            at: link.linked_at,
            how: AdmittedBy::Unsaid,
            left_at: None,
            told_by: None,
            told_with: Vec::new(),
        });
        let here: Vec<Admission> = devices.chain(links).collect();
        let admitting = std::mem::take(&mut self.admitting);
        for record in self.admissions.iter_mut().filter(|record| record.open()) {
            let still = (here.iter()).find(|one| one.key == record.key && one.host == record.host);
            match still {
                Some(one) => {
                    record.name.clone_from(&one.name);
                    record.platform.clone_from(&one.platform);
                }
                None => record.left_at = Some(now),
            }
        }
        let mut news = Vec::new();
        for mut one in here {
            let recorded = (self.admissions.iter())
                .any(|record| record.open() && record.key == one.key && record.host == one.host);
            if recorded {
                continue;
            }
            if let Some(how) = admitting.get(&one.key) {
                one.how = *how;
            }
            let tell = self.note_told(&one.key) && one.how == AdmittedBy::Ticket;
            if tell {
                one.told_with = self.take_listed(&one.key);
            }
            if one.how != AdmittedBy::Unsaid {
                news.push(News {
                    admission: one.clone(),
                    tell,
                });
            }
            self.keep_admission(one);
        }
        news
    }

    /// Past the limit, the oldest that left goes first; one still here is never dropped.
    fn keep_admission(&mut self, admission: Admission) {
        self.admissions.push(admission);
        if self.admissions.len() > KEPT {
            if let Some(oldest) = self.admissions.iter().position(|record| !record.open()) {
                self.admissions.remove(oldest);
            }
        }
    }

    /// If the account listed `key` when this host signed in, the names of the rest it listed,
    /// which the person is told of along with it.
    fn take_listed(&mut self, key: &str) -> Vec<String> {
        if !self.listed.contains_key(key) {
            return Vec::new();
        }
        (std::mem::take(&mut self.listed).into_iter())
            .filter(|(other, _)| self.note_told(other))
            .map(|(_, name)| name)
            .collect()
    }

    /// Marks what the linked hosts said of `key` as left, now that it is off the account or the
    /// person removed it.
    pub(super) fn note_gone(&mut self, key: &str, now: u64) {
        self.listed.remove(key);
        let copies = (self.admissions.iter_mut())
            .filter(|record| record.key == key && record.told_by.is_some());
        for copy in copies {
            copy.left_at.get_or_insert(now);
        }
    }

    /// Notes that the person was told of `key`, and whether that is the first time.
    fn note_told(&mut self, key: &str) -> bool {
        if self.told.iter().any(|told| told == key) {
            return false;
        }
        self.told.push_back(key.to_owned());
        if self.told.len() > TOLD_KEPT {
            self.told.pop_front();
        }
        true
    }
}

impl Remote {
    /// Everyone let in, newest first.
    pub fn admissions(&self) -> Vec<Admission> {
        let mut admissions = self.inner.locked().stored.admissions.clone();
        admissions.sort_by_key(|admission| std::cmp::Reverse(admission.at));
        admissions
    }

    /// Notes that the person hears of `key` now, and whether they had not yet.
    pub fn first_told(&self, key: &str) -> bool {
        self.change(|stored| Ok(stored.note_told(key)))
            .unwrap_or(true)
    }

    /// Notes what the account listed as this host signed in, by key and name.
    pub fn note_listed(&self, listed: Vec<(String, String)>) -> CoreResult<()> {
        let own = self.core_id();
        self.change(|stored| {
            stored.listed = (listed.into_iter())
                .filter(|(key, _)| own.as_ref() != Some(key))
                .map(|(key, name)| (key, clean(&name, NAME_LIMIT)))
                .collect();
            Ok(())
        })
    }
}

/// Tells the person and the linked hosts of everyone let in since they were last told.
pub fn tell_admitted(core: &Core) {
    let news = std::mem::take(&mut core.remote().inner.locked().news);
    for News { admission, tell } in news {
        if tell {
            say(core, &admission);
        }
        (core.remote_above().hosts_admitted)(core, &admission);
    }
}

/// The linked host named `host` let in `admission`: keeps a copy, and tells the person unless
/// they know of it.
pub fn heard_admitted(core: &Core, host: String, admission: Admission) -> CoreResult<()> {
    if core.remote().core_id().as_ref() == Some(&admission.key) {
        return Ok(());
    }
    let copy = Admission {
        key: clean(&admission.key, NAME_LIMIT),
        name: clean(&admission.name, NAME_LIMIT),
        platform: clean(&admission.platform, PLATFORM_LIMIT),
        told_by: Some(host),
        left_at: None,
        told_with: Vec::new(),
        ..admission
    };
    let first = core.remote().change(|stored| {
        (stored.admissions).retain(|kept| kept.key != copy.key || kept.told_by != copy.told_by);
        stored.keep_admission(copy.clone());
        Ok(stored.note_told(&copy.key))
    })?;
    if first && copy.how == AdmittedBy::Ticket {
        say(core, &copy);
    }
    core.changed(What::Read(Topic::Hosts));
    Ok(())
}

fn say(core: &Core, admission: &Admission) {
    core.changed(What::Event(&Event::AccountDevice {
        key: admission.key.clone(),
        name: admission.name.clone(),
        platform: admission.platform.clone(),
    }));
}

trait Open {
    fn open(&self) -> bool;
}

impl Open for Admission {
    /// On this host now, rather than gone or only heard of from another.
    fn open(&self) -> bool {
        self.told_by.is_none() && self.left_at.is_none()
    }
}
