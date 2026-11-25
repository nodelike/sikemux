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

/// What the org's key sealed for a host is bound to. Its first version also named keys sealed for
/// hosts nobody let in, so no key sealed under that is taken.
pub fn wrap_context(org: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-org-vault-key|v2|{org}|{generation}|{holder}")
}

/// Every host the org's vault lists, with its account's membership in the org.
pub fn listed(vault: &OrgVault) -> Option<BTreeMap<&str, &str>> {
    Some(
        (vault.hosts.as_ref()?.iter())
            .map(|host| (host.host_key.as_str(), host.member.as_str()))
            .collect(),
    )
}

/// The most computers of one account trusted in an org.
pub const COMPUTERS: usize = 50;
/// The most computers trusted in an org. With the longest names taken, the secret store's item
/// holds this many and the most remembered dropped.
const ROOM: usize = 3000;
/// The longest host key or membership trusted.
const NAME: usize = 64;

fn takes_name(name: &str) -> bool {
    let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_-".contains(&byte);
    name.len() <= NAME && name.bytes().all(plain)
}

/// The hosts that may come to be trusted: the first of each account's, in the order they were linked,
/// which is how the vault lists them. So every host keeps the same ones, and a host linked later
/// never takes an earlier one's place.
pub fn placed(vault: &OrgVault) -> Option<BTreeMap<&str, &str>> {
    let listed = listed(vault)?;
    let mut placed = BTreeMap::new();
    let mut of: BTreeMap<&str, usize> = BTreeMap::new();
    for host in vault.hosts.as_ref()? {
        let (key, member) = (host.host_key.as_str(), host.member.as_str());
        let of = of.entry(member).or_default();
        let counts = listed.get(key) == Some(&member) && takes_name(key) && takes_name(member);
        if counts && *of < COMPUTERS && placed.insert(key, member).is_none() {
            *of += 1;
        }
    }
    Some(placed)
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

    /// Whether there is room to trust `host` under `member`.
    pub fn takes(&self, host: &str, member: &str) -> bool {
        let of = || self.hosts.values().filter(|pin| pin.member == member);
        self.holds(host, member) || (self.hosts.len() < ROOM && of().count() < COMPUTERS)
    }

    /// As the secret store keeps it.
    pub fn text(&self) -> serde_json::Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }

    pub fn read(text: &str) -> Option<Pins> {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(text.trim()).ok()?).ok()
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
    /// longer lists under the membership it was trusted under. A host that left
    /// meanwhile still vouches for what it signed before, as nothing here heard it leave. A list
    /// without this host itself says nothing of the others. None trusted so far makes way for
    /// another.
    pub fn refresh(&mut self, org: &str, me: &str, vault: &OrgVault, now: i64) {
        let (Some(listed), Some(placed)) = (listed(vault), placed(vault)) else {
            return;
        };
        if !listed.contains_key(me) {
            return;
        }
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
                        && placed.get(vouch.host_key.as_str()) == Some(&vouch.member.as_str())
                        && (self.hosts.get(&vouch.host_key)).is_none_or(|pin| pin.at < vouch.at)
                        && (self.holds(&vouch.by, &vouch.by_member)
                            || before.holds(&vouch.by, &vouch.by_member))
                        && vouch.at > before.floor
                        && self.takes(&vouch.host_key, &vouch.member)
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
                if self.takes(&vouch.host_key, &vouch.member) {
                    self.pin(&vouch.host_key, &vouch.member, at);
                }
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

    /// With what each host that was given this one's code told it. What is told counts once
    /// `refresh` has read the same list, so a list without this host is passed over.
    pub fn introduced(&mut self, org: &str, me: &Me, vault: &OrgVault, secret: &Secret, now: i64) {
        let (Some(mut unplaced), Some(placed)) = (listed(vault), placed(vault)) else {
            return;
        };
        if !unplaced.contains_key(me.key.as_str()) {
            return;
        }
        // One the vault lists no longer is still taken in, for `refresh` to drop.
        unplaced.retain(|host, _| !placed.contains_key(host));
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
        for mut theirs in told {
            let (first, rest): (BTreeMap<_, _>, _) = std::mem::take(&mut theirs.hosts)
                .into_iter()
                .filter(|(host, _)| !unplaced.contains_key(host.as_str()))
                .partition(|(host, _)| placed.contains_key(host.as_str()));
            self.join(theirs.dropped, first.into_iter().chain(rest), &me.key, now);
        }
    }

    fn join(
        &mut self,
        dropped: BTreeMap<String, i64>,
        hosts: impl Iterator<Item = (String, Pin)>,
        me: &str,
        now: i64,
    ) {
        for (place, at) in dropped {
            let names = place.split_once('|');
            let names = names.is_some_and(|(host, of)| takes_name(host) && takes_name(of));
            if at <= now + AHEAD && names {
                let dropped = self.dropped.entry(place).or_default();
                *dropped = at.max(*dropped);
            }
        }
        self.forget_oldest_dropped();
        for (host, pin) in hosts {
            let held = self.hosts.get(&host);
            let newer = held.is_none_or(|held| held.at < pin.at);
            let names = takes_name(&host) && takes_name(&pin.member);
            if host != me
                && pin.at <= now + AHEAD
                && newer
                && names
                && self.takes(&host, &pin.member)
            {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_once_slow_brings_back_nothing_signed_before() {
        let now = 1_800_000_000_000;
        let slow = now - 365 * 24 * 60 * 60 * 1000;
        let vault = serde_json::from_value(serde_json::json!({
            "generation": 1, "key": null, "holders": [], "waiting": [], "rotate": false,
            "remadeAt": null, "entries": [], "ended": [], "changes": 0, "since": null,
            "hosts": [], "vouches": [],
        }))
        .unwrap();
        let mut pins = Pins::default().cleared(now);
        pins.refresh("org", "me", &vault, slow);
        assert_eq!(pins.floor, now);
        assert_eq!(pins.cleared(slow).floor, now);
    }

    const NOW: i64 = 1_800_000_000_000;

    /// The vault as it lists `hosts` in order, with a vouch by `by` for each of `vouched`.
    fn vault(hosts: &[(&Me, &str)], by: (&Me, &str), vouched: &[(&Me, &str)]) -> OrgVault {
        let hosts: Vec<_> = (hosts.iter())
            .map(|(host, member)| json!({ "hostKey": host.key, "member": member }))
            .collect();
        let vouches: Vec<_> = (vouched.iter().enumerate())
            .map(|(at, (host, member))| {
                // The later linked, the earlier dated.
                let at = NOW - i64::try_from(at).unwrap();
                let put = vouch("org", by, (&host.key, member), at).unwrap();
                json!({
                    "hostKey": host.key, "member": member, "by": by.0.key, "byMember": by.1,
                    "at": at, "signature": put.signature, "introduction": null,
                })
            })
            .collect();
        serde_json::from_value(json!({
            "generation": 1, "key": null, "holders": [], "waiting": [], "rotate": false,
            "remadeAt": null, "entries": [], "ended": [], "changes": 0, "since": null,
            "hosts": hosts, "vouches": vouches,
        }))
        .unwrap()
    }

    #[test]
    fn every_host_trusts_the_same_fifty_computers_of_an_account() {
        let host = || Me::of(&iroh::SecretKey::generate());
        let (me, other) = (host(), host());
        let theirs: Vec<Me> = (0..COMPUTERS + 20).map(|_| host()).collect();
        let mut hosts = vec![(&me, "manager"), (&other, "manager")];
        hosts.extend(theirs.iter().map(|host| (host, "member")));
        let (by, all) = ((&theirs[0], "member"), &hosts[3..]);
        let reads = |pins: &mut Pins, me: &Me, hosts: &[(&Me, &str)], vouched: &[(&Me, &str)]| {
            pins.refresh("org", &me.key, &vault(hosts, by, vouched), NOW);
        };
        let first = |pins: &Pins, from: usize, to: usize| {
            theirs
                .iter()
                .enumerate()
                .all(|(at, host)| pins.hold(&host.key) == (from..to).contains(&at))
        };

        let (mut mine, mut others) = (Pins::default(), Pins::default());
        mine.let_in(&theirs[0].key, "member", 1);
        others.let_in(&theirs[0].key, "member", 1);
        reads(&mut mine, &me, &hosts, &all[..10]);
        reads(&mut mine, &me, &hosts, all);
        for at in (0..all.len()).rev() {
            reads(&mut others, &other, &hosts, &all[at..]);
        }
        assert!(first(&mine, 0, COMPUTERS));
        assert!(first(&others, 0, COMPUTERS));

        let gone: Vec<(&Me, &str)> = [&hosts[..4], &hosts[6..]].concat();
        reads(&mut mine, &me, &gone, all);
        assert!((2..4).all(|at| !mine.hold(&theirs[at].key)));
        assert!((4..COMPUTERS + 2).all(|at| mine.hold(&theirs[at].key)));
        assert!(!mine.hold(&theirs[COMPUTERS + 2].key));

        let mut told = Pins::default();
        for at in [0, COMPUTERS - 1, COMPUTERS] {
            told.let_in(&theirs[at].key, "member", 1);
        }
        let (secret, seen) = ([7; 8], vault(&hosts, by, &[]));
        let to = (me.key.as_str(), "manager");
        let word = introduce("org", &other, &seen, &told, to, NOW, &secret).unwrap();
        let mut listing = vault(&hosts, by, &[]);
        listing.vouches = Some(vec![OrgVaultVouch {
            host_key: me.key.clone(),
            member: "manager".to_owned(),
            by: other.key.clone(),
            by_member: "manager".to_owned(),
            at: word.at,
            signature: word.signature,
            introduction: word.introduction,
        }]);
        let mut new = Pins::default();
        new.introduced("org", &me, &listing, &secret, NOW);
        assert!(new.hold(&other.key));
        assert!(new.hold(&theirs[COMPUTERS - 1].key));
        assert!(!new.hold(&theirs[COMPUTERS].key));
    }

    #[test]
    fn what_a_host_trusts_always_fits_its_secret_store() {
        let host = || Me::of(&iroh::SecretKey::generate());
        let (me, by) = (host(), host());
        let hosts = |count: usize| -> Vec<(Me, String)> {
            (0..count)
                .map(|at| (host(), format!("{:0>1$}", at / COMPUTERS, NAME)))
                .collect()
        };
        let mut pins = Pins::default();
        pins.let_in(&by.key, "by", NOW);
        let mut read = |of: &[(Me, String)]| {
            let mut hosts = vec![(&me, "me"), (&by, "by")];
            hosts.extend(of.iter().map(|(host, member)| (host, member.as_str())));
            pins.refresh(
                "org",
                &me.key,
                &vault(&hosts, (&by, "by"), &hosts[2..]),
                NOW,
            );
            pins.clone()
        };

        read(&hosts(DROPPED));
        let some = hosts(ROOM / 2);
        read(&some);
        let mut more = hosts(ROOM);
        more.insert(0, (host(), "\u{1}".repeat(NAME)));
        more.splice(0..0, some);
        let pins = read(&more);
        assert_eq!((pins.hosts.len(), pins.dropped.len()), (ROOM, DROPPED));
        assert!(more[..ROOM / 2]
            .iter()
            .all(|(host, _)| pins.hold(&host.key)));
        assert!(!pins.hold(&more[ROOM / 2].0.key));
        assert!(pins.text().unwrap().len() <= sikemux_keychain::SECRET);
    }
}
