//! The vault's keys. The vault key seals every entry; each host gets it wrapped for its own key,
//! the Ed25519 key it joined the account with, taken as the X25519 key it converts to. The
//! recovery key is an X25519 key the person keeps, and the vault key is wrapped for it as well.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use curve25519_dalek::edwards::CompressedEdwardsY;
use curve25519_dalek::montgomery::MontgomeryPoint;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::collections::BTreeMap;

pub const KEY_LEN: usize = 32;

pub type Key = [u8; KEY_LEN];

pub fn random_key() -> Option<Key> {
    let mut key = [0; KEY_LEN];
    SystemRandom::new().fill(&mut key).ok()?;
    Some(key)
}

/// A host's X25519 secret, from its Ed25519 key as signing derives its scalar.
pub fn host_secret(host: &iroh::SecretKey) -> Key {
    let hashed = Sha512::digest(host.to_bytes());
    let mut scalar = [0; KEY_LEN];
    for (to, from) in scalar.iter_mut().zip(hashed.iter()) {
        *to = *from;
    }
    scalar
}

/// A host's X25519 public key, from the Ed25519 key in lowercase hex the account knows it by.
pub fn host_public(key: &str) -> Option<MontgomeryPoint> {
    let bytes: Key = hex::decode(key).ok()?.try_into().ok()?;
    Some(CompressedEdwardsY(bytes).decompress()?.to_montgomery())
}

pub fn public_of(secret: &Key) -> MontgomeryPoint {
    MontgomeryPoint::mul_base_clamped(*secret)
}

pub fn recovery_public(key: &str) -> Option<MontgomeryPoint> {
    let bytes: Key = hex::decode(key).ok()?.try_into().ok()?;
    Some(MontgomeryPoint(bytes))
}

struct Length;

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        KEY_LEN
    }
}

fn aead_key(shared: &[u8], context: &str, info: &[&[u8]]) -> Option<LessSafeKey> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, context.as_bytes());
    let prk = salt.extract(shared);
    let okm = prk.expand(info, Length).ok()?;
    let mut key = [0; KEY_LEN];
    okm.fill(&mut key).ok()?;
    Some(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).ok()?))
}

fn seal_with(key: &LessSafeKey, aad: &str, plain: &[u8]) -> Option<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).ok()?;
    let mut sealed = plain.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad.as_bytes()),
        &mut sealed,
    )
    .ok()?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    Some(out)
}

fn open_with(key: &LessSafeKey, aad: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < NONCE_LEN {
        return None;
    }
    let (nonce, rest) = bytes.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut rest = rest.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::from(aad.as_bytes()), &mut rest)
        .ok()?;
    Some(plain.to_vec())
}

/// What a wrapped vault key is bound to, so a copy cannot pass for another account's,
/// generation's or holder's.
pub fn wrap_context(account: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-vault-key|v1|{account}|{generation}|{holder}")
}

/// The vault key sealed for the holder of `to`'s secret, with a key made for this once.
pub fn wrap(vault: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    let ephemeral = random_key()?;
    let ephemeral_public = public_of(&ephemeral);
    let shared = to.mul_clamped(ephemeral);
    let key = aead_key(shared.as_bytes(), context, &[ephemeral_public.as_bytes()])?;
    let mut out = ephemeral_public.as_bytes().to_vec();
    out.extend(seal_with(&key, context, vault)?);
    Some(URL_SAFE_NO_PAD.encode(out))
}

pub fn unwrap(wrapped: &str, secret: &Key, context: &str) -> Option<Key> {
    let bytes = URL_SAFE_NO_PAD.decode(wrapped).ok()?;
    if bytes.len() < KEY_LEN {
        return None;
    }
    let (ephemeral, sealed) = bytes.split_at(KEY_LEN);
    let ephemeral = MontgomeryPoint(ephemeral.try_into().ok()?);
    let shared = ephemeral.mul_clamped(*secret);
    let key = aead_key(shared.as_bytes(), context, &[ephemeral.as_bytes()])?;
    open_with(&key, context, sealed)?.try_into().ok()
}

/// The vault key sealed for `to` so that only the holder of `from`'s secret could have sealed
/// it: the sealer's public key leads the wrap, and its own key is mixed into the sealing key.
pub fn wrap_from(vault: &Key, from: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    let sender = public_of(from);
    let ephemeral = random_key()?;
    let ephemeral_public = public_of(&ephemeral);
    let shared = [to.mul_clamped(ephemeral).0, to.mul_clamped(*from).0].concat();
    let key = aead_key(
        &shared,
        context,
        &[sender.as_bytes(), ephemeral_public.as_bytes()],
    )?;
    let mut out = sender.as_bytes().to_vec();
    out.extend_from_slice(ephemeral_public.as_bytes());
    out.extend(seal_with(&key, context, vault)?);
    Some(URL_SAFE_NO_PAD.encode(out))
}

/// Opens a wrap made by [`wrap_from`], with the public key of whoever sealed it, which the
/// caller decides whether to trust.
pub fn unwrap_from(wrapped: &str, secret: &Key, context: &str) -> Option<(Key, MontgomeryPoint)> {
    let bytes = URL_SAFE_NO_PAD.decode(wrapped).ok()?;
    if bytes.len() < 2 * KEY_LEN {
        return None;
    }
    let (sender, rest) = bytes.split_at(KEY_LEN);
    let (ephemeral, sealed) = rest.split_at(KEY_LEN);
    let sender = MontgomeryPoint(sender.try_into().ok()?);
    let ephemeral = MontgomeryPoint(ephemeral.try_into().ok()?);
    let shared = [
        ephemeral.mul_clamped(*secret).0,
        sender.mul_clamped(*secret).0,
    ]
    .concat();
    let key = aead_key(&shared, context, &[sender.as_bytes(), ephemeral.as_bytes()])?;
    let vault = open_with(&key, context, sealed)?.try_into().ok()?;
    Some((vault, sender))
}

/// The newest of a vault this host has taken in, so a server handing back an older generation or
/// an older copy of an entry is refused, and a removed entry cannot come back as it was.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Seen {
    pub generation: i64,
    revisions: BTreeMap<String, i64>,
    removed: BTreeMap<String, i64>,
}

impl Seen {
    pub fn takes_generation(&self, generation: i64) -> bool {
        generation >= self.generation
    }

    pub fn takes_entry(&self, id: &str, revision: i64) -> bool {
        self.revisions.get(id).is_none_or(|seen| revision >= *seen)
            && self.removed.get(id).is_none_or(|gone| revision > *gone)
    }

    /// Takes in a vault at `generation` holding exactly `entries`, as id and revision, each of
    /// which this took.
    pub fn note<'a>(&mut self, generation: i64, entries: impl IntoIterator<Item = (&'a str, i64)>) {
        self.generation = self.generation.max(generation);
        let live: BTreeMap<&str, i64> = entries.into_iter().collect();
        let revisions = std::mem::take(&mut self.revisions);
        for (id, revision) in revisions {
            if !live.contains_key(id.as_str()) {
                self.removed.insert(id, revision);
            }
        }
        for (id, revision) in live {
            self.removed.remove(id);
            self.revisions.insert(id.to_owned(), revision);
        }
    }
}

/// What a sealed entry is bound to, so the server cannot hand back an older copy of it.
#[derive(Clone, Copy)]
pub struct Place<'a> {
    pub id: &'a str,
    pub generation: i64,
    pub revision: i64,
}

fn entry_aad(place: Place) -> String {
    let Place {
        id,
        generation,
        revision,
    } = place;
    format!("sikemux-vault-entry|v1|{id}|{generation}|{revision}")
}

pub fn seal_entry(vault: &Key, place: Place, plain: &[u8]) -> Option<String> {
    seal_bound(vault, &entry_aad(place), plain)
}

pub fn open_entry(vault: &Key, place: Place, sealed: &str) -> Option<Vec<u8>> {
    open_bound(vault, &entry_aad(place), sealed)
}

/// Seals `plain` under the vault key, bound to `context`, which opening must name again.
pub fn seal_bound(vault: &Key, context: &str, plain: &[u8]) -> Option<String> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, vault).ok()?);
    Some(URL_SAFE_NO_PAD.encode(seal_with(&key, context, plain)?))
}

pub fn open_bound(vault: &Key, context: &str, sealed: &str) -> Option<Vec<u8>> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, vault).ok()?);
    let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
    open_with(&key, context, &bytes)
}

/// Seals `plain` under `key`, bound to `context`, as bytes rather than text.
pub fn seal_bytes(key: &Key, context: &str, plain: &[u8]) -> Option<Vec<u8>> {
    seal_with(
        &LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?),
        context,
        plain,
    )
}

pub fn open_bytes(key: &Key, context: &str, sealed: &[u8]) -> Option<Vec<u8>> {
    open_with(
        &LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?),
        context,
        sealed,
    )
}

/// Crockford's base32, which leaves out letters that read like digits.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The recovery key as the person writes it down: its 256 bits in base32, four characters a
/// group.
pub fn recovery_text(secret: &Key) -> String {
    let mut digits: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in secret {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            digits.extend(RECOVERY_ALPHABET.get(((buffer >> bits) & 31) as usize));
        }
    }
    if bits > 0 {
        digits.extend(RECOVERY_ALPHABET.get(((buffer << (5 - bits)) & 31) as usize));
    }
    digits
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Reads a recovery key as the person typed it, in any case and with any separators.
pub fn recovery_secret(typed: &str) -> Option<Key> {
    let mut out = Vec::with_capacity(KEY_LEN);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in typed.chars().filter(char::is_ascii_alphanumeric) {
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        let value = RECOVERY_ALPHABET.iter().position(|&d| d == c as u8)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    out.truncate(KEY_LEN);
    out.try_into().ok()
}
