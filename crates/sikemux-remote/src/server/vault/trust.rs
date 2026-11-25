//! Which hosts this host hands an org's key to and takes it from (ADR 0026): those it let in by
//! a code, and those they vouch for. All of it is checked here, since the accounts server only
//! passes it along.

use std::collections::BTreeMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::accounts::orgvault::{OrgVault, OrgVaultVouch, OrgVaultVouchPut};

use super::seal::{self, Key};

/// This host, as its key signs and opens.
pub struct Me {
    pub key: String,
    pub secret: Key,
    host: iroh::SecretKey,
}

impl Me {
    pub fn of(host: &iroh::SecretKey) -> Me {
        Me {
            key: host.public().to_string(),
            secret: seal::host_secret(host),
            host: host.clone(),
        }
    }
}

/// What the org's key sealed for a host is bound to.
pub fn wrap_context(org: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-org-vault-key|v1|{org}|{generation}|{holder}")
}

/// Every host the org's vault lists, with its account's membership in the org.
pub fn listed(vault: &OrgVault) -> Option<BTreeMap<&str, &str>> {
    Some(
        (vault.hosts.as_ref()?.iter())
            .map(|host| (host.host_key.as_str(), host.member.as_str()))
            .collect(),
    )
}

/// How far past this host's clock another's may be before what it dates counts for nothing.
const AHEAD: i64 = 24 * 60 * 60 * 1000;
/// The most dropped hosts remembered, the latest of them.
const DROPPED: usize = 300;
/// The most trusted hosts told to a host let in. It reaches the rest by vouches.
const TOLD: usize = 500;

/// A host trusted with the org's key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pin {
    /// The membership it was trusted under.
    member: String,
    /// When someone let it in or vouched for it, in milliseconds since 1970.
    at: i64,
}

/// The hosts this host trusts with the org's key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pins {
    #[serde(default)]
    hosts: BTreeMap<String, Pin>,
    /// When each host under a membership was last dropped, in milliseconds since 1970. Only what
    /// was signed of it, or by it, later counts.
    #[serde(default)]
    dropped: BTreeMap<String, i64>,
    /// Nothing signed of any host up to here counts, in milliseconds since 1970.
    #[serde(default)]
    floor: i64,
}

fn under(host: &str, member: &str) -> String {
    format!("{host}|{member}")
}

impl Pins {
    pub fn hold(&self, host: &str) -> bool {
        self.hosts.contains_key(host)
    }

    fn holds(&self, host: &str, member: &str) -> bool {
        (self.hosts.get(host)).is_some_and(|pin| pin.member == member)
    }

    /// Trusting no host, nor anything signed of one until now.
    pub fn cleared(&self, now: i64) -> Pins {
        Pins {
            hosts: BTreeMap::new(),
            dropped: self.dropped.clone(),
            floor: now.max(self.floor),
        }
    }

    fn pin(&mut self, host: &str, member: &str, at: i64) {
        let pin = Pin {
            member: member.to_owned(),
            at,
        };
        self.hosts.insert(host.to_owned(), pin);
    }

    fn since_dropped(&self, host: &str, member: &str, at: i64) -> bool {
        (self.dropped.get(&under(host, member))).is_none_or(|dropped| at > *dropped)
    }

    fn after_drop(&self, host: &str, member: &str, at: i64) -> i64 {
        let dropped = self.dropped.get(&under(host, member));
        dropped.map_or(at, |dropped| at.max(dropped + 1))
    }

    fn forget_oldest_dropped(&mut self) {
        while self.dropped.len() > DROPPED {
            let oldest = (self.dropped.iter())
                .min_by_key(|(_, at)| **at)
                .map(|(place, _)| place.clone());
            let at = oldest.and_then(|oldest| self.dropped.remove(&oldest));
            self.floor = self.floor.max(at.unwrap_or_default());
        }
    }

    /// Trusts `host` on a code the person pasted, and says when, later than it was ever dropped.
    pub fn let_in(&mut self, host: &str, member: &str, now: i64) -> i64 {
        let mut at = self.after_drop(host, member, now).max(self.floor + 1);
        // Other hosts would not take a time this far off, left by a clock that was wrong once.
        if at > now + AHEAD {
            self.dropped.remove(&under(host, member));
            at = now;
        }
        self.pin(host, member, at);
        at
    }

    /// Takes in the hosts that those trusted so far vouch for, then drops each the vault no
    /// longer lists under the membership it was trusted under. A host that left meanwhile still
    /// vouches for what it signed before, as nothing here heard it leave.
    pub fn refresh(&mut self, org: &str, me: &str, vault: &OrgVault, now: i64) {
        let Some(listed) = listed(vault) else { return };
        let before = self.clone();
        let (dropped, hosts) = (&mut self.dropped, &mut self.hosts);
        hosts.retain(|host, pin| {
            let stays = listed.get(host.as_str()) == Some(&pin.member.as_str());
            if !stays {
                let last = dropped.entry(under(host, &pin.member)).or_default();
                *last = now.max(pin.at).max(*last);
            }
            stays
        });
        self.forget_oldest_dropped();
        let vouches = vault.vouches.as_deref().unwrap_or_default();
        loop {
            let reached: Vec<&OrgVaultVouch> = (vouches.iter())
                .filter(|vouch| {
                    vouch.host_key != me
                        && listed.get(vouch.host_key.as_str()) == Some(&vouch.member.as_str())
                        && (self.hosts.get(&vouch.host_key)).is_none_or(|pin| pin.at < vouch.at)
                        && (self.holds(&vouch.by, &vouch.by_member)
                            || before.holds(&vouch.by, &vouch.by_member))
                        && vouch.at > before.floor
                        && before.counts(org, vouch, now)
                })
                .collect();
            if reached.is_empty() {
                return;
            }
            for vouch in reached {
                let at = self
                    .hosts
                    .get(&vouch.host_key)
                    .map_or(vouch.at, |pin| pin.at.max(vouch.at));
                self.pin(&vouch.host_key, &vouch.member, at);
            }
        }
    }

    /// Its signer signed it, and since this host last dropped either host.
    fn counts(&self, org: &str, vouch: &OrgVaultVouch, now: i64) -> bool {
        vouch.at <= now + AHEAD
            && self.since_dropped(&vouch.host_key, &vouch.member, vouch.at)
            && self.since_dropped(&vouch.by, &vouch.by_member, vouch.at)
            && vouched(org, vouch)
    }

    /// The trusted hosts that nothing this host trusts vouches for on the server, for this host
    /// to: any for an owner's or admin's, else its own account's and those that vouched for it.
    pub fn unvouched(
        &self,
        org: &str,
        me: &Me,
        vault: &OrgVault,
        manages: bool,
        now: i64,
    ) -> Vec<OrgVaultVouchPut> {
        let Some(listed) = listed(vault) else {
            return Vec::new();
        };
        let mine = listed.get(me.key.as_str()).copied();
        let vouches = vault.vouches.as_deref().unwrap_or_default();
        (self.hosts.iter())
            .filter(|(host, pin)| {
                manages
                    || mine == Some(pin.member.as_str())
                    || (vouches.iter()).any(|vouch| vouch.host_key == me.key && &vouch.by == *host)
            })
            .filter(|(host, pin)| {
                !vouches.iter().any(|vouch| {
                    let trusted = match vouch.by == me.key {
                        true => mine == Some(vouch.by_member.as_str()),
                        false => self.holds(&vouch.by, &vouch.by_member),
                    };
                    &vouch.host_key == *host
                        && vouch.member == pin.member
                        && vouch.at >= pin.at
                        && trusted
                        && self.counts(org, vouch, now)
                })
            })
            .filter_map(|(host, pin)| {
                let mine = mine?;
                let at = self.after_drop(&me.key, mine, pin.at);
                vouch(org, (me, mine), (host, &pin.member), at)
            })
            .collect()
    }

    /// With what each host that was given this one's code told it.
    pub fn introduced(&mut self, org: &str, me: &Me, vault: &OrgVault, secret: &Secret, now: i64) {
        let told: Vec<Pins> = (vault.vouches.iter().flatten())
            .filter(|vouch| vouch.host_key == me.key)
            .filter_map(|vouch| {
                let sealed = vouch.introduction.as_ref()?;
                open_introduction(sealed, org, me, vouch, secret)
            })
            .collect();
        let floor = (told.iter().map(|theirs| theirs.floor))
            .filter(|floor| *floor <= now + AHEAD)
            .fold(self.floor, i64::max);
        if floor > self.floor {
            self.floor = floor;
            self.hosts.retain(|_, pin| pin.at > floor);
        }
        for theirs in told {
            self.join(theirs, &me.key, now);
        }
    }

    fn join(&mut self, theirs: Pins, me: &str, now: i64) {
        for (place, at) in theirs.dropped {
            if at <= now + AHEAD {
                let dropped = self.dropped.entry(place).or_default();
                *dropped = at.max(*dropped);
            }
        }
        self.forget_oldest_dropped();
        for (host, pin) in theirs.hosts {
            let held = self.hosts.get(&host);
            if host != me && pin.at <= now + AHEAD && held.is_none_or(|held| held.at < pin.at) {
                self.hosts.insert(host, pin);
            }
        }
        let dropped = &self.dropped;
        (self.hosts).retain(|host, pin| {
            (dropped.get(&under(host, &pin.member))).is_none_or(|dropped| pin.at > *dropped)
        });
    }
}

/// Text that no other list of parts spells.
fn bound(parts: &[&str]) -> String {
    json!(parts).to_string()
}

fn vouch_text(org: &str, by: (&str, &str), host: (&str, &str), at: i64) -> String {
    bound(&[
        "sikemux-org-vault-vouch-v1",
        org,
        by.0,
        by.1,
        host.0,
        host.1,
        &at.to_string(),
    ])
}

fn vouched(org: &str, vouch: &OrgVaultVouch) -> bool {
    let signed = || {
        let by: iroh::PublicKey = vouch.by.parse().ok()?;
        let bytes = URL_SAFE_NO_PAD.decode(&vouch.signature).ok()?;
        let signature = iroh::Signature::from_bytes(&bytes.try_into().ok()?);
        let text = vouch_text(
            org,
            (&vouch.by, &vouch.by_member),
            (&vouch.host_key, &vouch.member),
            vouch.at,
        );
        by.verify(text.as_bytes(), &signature).ok()
    };
    signed().is_some()
}

/// This host's word that `host`, under membership `member`, may have the key. Its own is `mine`.
fn vouch(
    org: &str,
    (me, mine): (&Me, &str),
    (host, member): (&str, &str),
    at: i64,
) -> Option<OrgVaultVouchPut> {
    let text = vouch_text(org, (&me.key, mine), (host, member), at);
    Some(OrgVaultVouchPut {
        host_key: host.to_owned(),
        at,
        signature: URL_SAFE_NO_PAD.encode(me.host.sign(text.as_bytes()).to_bytes()),
        introduction: None,
    })
}

/// The half of a code the accounts server never sees.
pub type Secret = [u8; 8];

/// How much of the fingerprint a code carries.
const FINGERPRINT: usize = 12;

pub fn new_secret() -> Option<Secret> {
    seal::random_key()?.get(..8)?.try_into().ok()
}

fn fingerprint(org: &str, host: &str, member: &str, secret: &Secret) -> Vec<u8> {
    let text = bound(&[
        "sikemux-org-vault-code-v1",
        org,
        host,
        member,
        &hex::encode(secret),
    ]);
    Sha256::digest(text.as_bytes())
        .into_iter()
        .take(FINGERPRINT)
        .collect()
}

/// What the waiting host shows for the person to paste on a computer that holds the key.
pub fn code(org: &str, host: &str, member: &str, secret: &Secret) -> String {
    seal::digits(&[secret.as_slice(), &fingerprint(org, host, member, secret)].concat())
}

/// A code as the person pasted it.
pub struct Code {
    pub secret: Secret,
    fingerprint: Vec<u8>,
}

impl Code {
    pub fn read(typed: &str) -> Option<Code> {
        let bytes = seal::digit_bytes(typed)?;
        let (secret, fingerprint) = bytes.split_at_checked(8)?;
        (fingerprint.len() == FINGERPRINT).then(|| Code {
            secret: secret.try_into().unwrap_or_default(),
            fingerprint: fingerprint.to_vec(),
        })
    }

    pub fn names(&self, org: &str, host: &str, member: &str) -> bool {
        self.fingerprint == fingerprint(org, host, member, &self.secret)
    }
}

/// What an introduction is sealed for, which only a host given the code can name.
fn introduction_context(
    org: &str,
    by: (&str, &str),
    host: (&str, &str),
    secret: &Secret,
) -> String {
    bound(&[
        "sikemux-org-vault-introduction-v1",
        org,
        by.0,
        by.1,
        host.0,
        host.1,
        &hex::encode(secret),
    ])
}

/// This host's vouch for `host`, let in at `at` by the code with `secret`. An introduction comes
/// with it: the hosts this one trusts, itself among them, sealed for that host alone. It stays good
/// whichever key the vault has by the time it is read.
pub fn introduce(
    org: &str,
    me: &Me,
    vault: &OrgVault,
    pins: &Pins,
    (host, member): (&str, &str),
    at: i64,
    secret: &Secret,
) -> Option<OrgVaultVouchPut> {
    let mine = *listed(vault)?.get(me.key.as_str())?;
    let at = pins.after_drop(&me.key, mine, at);
    let mut told = Pins {
        hosts: (pins.hosts.iter())
            .take(TOLD)
            .map(|(host, pin)| (host.clone(), pin.clone()))
            .collect(),
        dropped: pins.dropped.clone(),
        floor: pins.floor,
    };
    told.pin(&me.key, mine, at);
    let context = introduction_context(org, (&me.key, mine), (host, member), secret);
    let sealed = seal::seal_from(
        &serde_json::to_vec(&told).ok()?,
        &me.secret,
        &seal::host_public(host)?,
        &context,
    )?;
    Some(OrgVaultVouchPut {
        introduction: Some(sealed),
        ..vouch(org, (me, mine), (host, member), at)?
    })
}

fn open_introduction(
    sealed: &str,
    org: &str,
    me: &Me,
    vouch: &OrgVaultVouch,
    secret: &Secret,
) -> Option<Pins> {
    let by = (vouch.by.as_str(), vouch.by_member.as_str());
    let context = introduction_context(org, by, (&me.key, &vouch.member), secret);
    let (plain, sender) = seal::open_from(sealed, &me.secret, &context)?;
    if seal::host_public(&vouch.by) != Some(sender) {
        return None;
    }
    serde_json::from_slice(&plain).ok()
}
