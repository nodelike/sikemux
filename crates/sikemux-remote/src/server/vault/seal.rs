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
use sha2::{Digest, Sha512};

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

/// What a vault key wrapped for a host by [`wrap_from`] is bound to, so a copy cannot pass for
/// another account's, generation's or holder's.
pub fn host_wrap_context(account: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-vault-key|v2|{account}|{generation}|{holder}")
}

fn dh(point: &MontgomeryPoint, secret: &Key) -> Option<Key> {
    let shared = point.mul_clamped(*secret).0;
    let non_zero = shared
        .iter()
        .fold(0u8, |acc, &byte| acc | std::hint::black_box(byte));
    (non_zero != 0).then_some(shared)
}

/// The vault key sealed for the holder of `to`'s secret, with a key made for this once.
pub fn wrap(vault: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    let ephemeral = random_key()?;
    let ephemeral_public = public_of(&ephemeral);
    let shared = dh(to, &ephemeral)?;
    let key = aead_key(&shared, context, &[ephemeral_public.as_bytes()])?;
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
    let shared = dh(&ephemeral, secret)?;
    let key = aead_key(&shared, context, &[ephemeral.as_bytes()])?;
    open_with(&key, context, sealed)?.try_into().ok()
}

/// The vault key sealed for `to` as [`seal_from`] does.
pub fn wrap_from(vault: &Key, from: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    seal_from(vault, from, to, context)
}

/// Opens a wrap made by [`wrap_from`], with the public key of whoever sealed it, which the
/// caller decides whether to trust.
pub fn unwrap_from(wrapped: &str, secret: &Key, context: &str) -> Option<(Key, MontgomeryPoint)> {
    let (vault, sender) = open_from(wrapped, secret, context)?;
    Some((vault.try_into().ok()?, sender))
}

/// `plain` sealed for `to` so that only the holder of `from`'s secret could have sealed it: the
/// sealer's public key leads, and its own key is mixed into the sealing key.
pub fn seal_from(plain: &[u8], from: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    let sender = public_of(from);
    let ephemeral = random_key()?;
    let ephemeral_public = public_of(&ephemeral);
    let shared = [dh(to, &ephemeral)?, dh(to, from)?].concat();
    let key = aead_key(
        &shared,
        context,
        &[sender.as_bytes(), ephemeral_public.as_bytes()],
    )?;
    let mut out = sender.as_bytes().to_vec();
    out.extend_from_slice(ephemeral_public.as_bytes());
    out.extend(seal_with(&key, context, plain)?);
    Some(URL_SAFE_NO_PAD.encode(out))
}

/// Opens what [`seal_from`] sealed, with the public key of whoever sealed it.
pub fn open_from(sealed: &str, secret: &Key, context: &str) -> Option<(Vec<u8>, MontgomeryPoint)> {
    let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
    if bytes.len() < 2 * KEY_LEN {
        return None;
    }
    let (sender, rest) = bytes.split_at(KEY_LEN);
    let (ephemeral, sealed) = rest.split_at(KEY_LEN);
    let sender = MontgomeryPoint(sender.try_into().ok()?);
    let ephemeral = MontgomeryPoint(ephemeral.try_into().ok()?);
    let shared = [dh(&ephemeral, secret)?, dh(&sender, secret)?].concat();
    let key = aead_key(&shared, context, &[sender.as_bytes(), ephemeral.as_bytes()])?;
    Some((open_with(&key, context, sealed)?, sender))
}

/// Opens a wrap made by [`wrap_from`] only if one of `holders`, hosts by their Ed25519 key in
/// hex, sealed it, so a server cannot hand over a key of its own making.
pub fn unwrap_from_holder<'a>(
    wrapped: &str,
    secret: &Key,
    context: &str,
    holders: impl IntoIterator<Item = &'a str>,
) -> Option<Key> {
    let (key, sender) = unwrap_from(wrapped, secret, context)?;
    (holders.into_iter())
        .filter_map(host_public)
        .any(|holder| holder == sender)
        .then_some(key)
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
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The recovery key as the person writes it down.
pub fn recovery_text(secret: &Key) -> String {
    digits(secret)
}

/// Reads a recovery key as the person typed it, in any case and with any separators.
pub fn recovery_secret(typed: &str) -> Option<Key> {
    let mut out = digit_bytes(typed)?;
    out.truncate(KEY_LEN);
    out.try_into().ok()
}

/// Bytes as a person copies them: in base32, four characters a group.
pub fn digits(bytes: &[u8]) -> String {
    let mut digits: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            digits.extend(ALPHABET.get(((buffer >> bits) & 31) as usize));
        }
    }
    if bits > 0 {
        digits.extend(ALPHABET.get(((buffer << (5 - bits)) & 31) as usize));
    }
    digits
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Reads [`digits`] back, in any case and with any separators.
pub fn digit_bytes(typed: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(KEY_LEN);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in typed.chars().filter(char::is_ascii_alphanumeric) {
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        let value = ALPHABET.iter().position(|&d| d == c as u8)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7748 §6.1 low-order X25519 points whose scalar multiplication yields all zeros.
    const LOW_ORDER_POINTS: [[u8; KEY_LEN]; 3] = [
        [0u8; KEY_LEN],
        {
            let mut p = [0u8; KEY_LEN];
            p[0] = 1;
            p
        },
        [
            0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f,
            0xc4, 0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16,
            0x5f, 0x49, 0xb8, 0x00,
        ],
    ];

    fn forge_wrap(ephemeral: [u8; KEY_LEN], vault: &Key, context: &str) -> String {
        let key = aead_key(&[0u8; KEY_LEN], context, &[&ephemeral]).unwrap();
        let mut out = ephemeral.to_vec();
        out.extend(seal_with(&key, context, vault).unwrap());
        URL_SAFE_NO_PAD.encode(out)
    }

    #[test]
    fn rejects_low_order_x25519_wrap_forgery() {
        let recipient_secret = random_key().unwrap();
        let forged_vault_key = random_key().unwrap();
        let context = wrap_context("user_1", 1, "host_1");

        for point in LOW_ORDER_POINTS {
            let forged = forge_wrap(point, &forged_vault_key, &context);
            assert!(
                unwrap(&forged, &recipient_secret, &context).is_none(),
                "unwrap must reject low-order ephemeral point"
            );
            assert!(
                wrap(&forged_vault_key, &MontgomeryPoint(point), &context).is_none(),
                "wrap must reject low-order recipient point"
            );
        }
    }

    #[test]
    fn rejects_low_order_x25519_in_seal_from_and_open_from() {
        let sender_secret = random_key().unwrap();
        let recipient_secret = random_key().unwrap();
        let sender_public = public_of(&sender_secret);
        let recipient_public = public_of(&recipient_secret);
        let vault_key = random_key().unwrap();
        let context = host_wrap_context("user_1", 1, "host_1");

        for point in LOW_ORDER_POINTS {
            assert!(
                seal_from(
                    &vault_key,
                    &sender_secret,
                    &MontgomeryPoint(point),
                    &context
                )
                .is_none(),
                "seal_from must reject low-order recipient point"
            );

            let real_static = sender_public.mul_clamped(recipient_secret).0;
            let shared_low_eph = [[0u8; KEY_LEN], real_static].concat();
            let aead = aead_key(
                &shared_low_eph,
                &context,
                &[sender_public.as_bytes(), &point],
            )
            .unwrap();
            let mut low_eph = sender_public.as_bytes().to_vec();
            low_eph.extend_from_slice(&point);
            low_eph.extend(seal_with(&aead, &context, &vault_key).unwrap());
            assert!(
                open_from(
                    &URL_SAFE_NO_PAD.encode(low_eph),
                    &recipient_secret,
                    &context
                )
                .is_none(),
                "open_from must reject low-order ephemeral point"
            );

            let eph_secret = random_key().unwrap();
            let eph_public = public_of(&eph_secret);
            let real_eph = recipient_public.mul_clamped(eph_secret).0;
            let shared_low_sender = [real_eph, [0u8; KEY_LEN]].concat();
            let aead = aead_key(
                &shared_low_sender,
                &context,
                &[&point, eph_public.as_bytes()],
            )
            .unwrap();
            let mut low_sender = point.to_vec();
            low_sender.extend_from_slice(eph_public.as_bytes());
            low_sender.extend(seal_with(&aead, &context, &vault_key).unwrap());
            assert!(
                open_from(
                    &URL_SAFE_NO_PAD.encode(low_sender),
                    &recipient_secret,
                    &context
                )
                .is_none(),
                "open_from must reject low-order sender point"
            );
        }
    }

    #[test]
    fn valid_wraps_round_trip() {
        let sender = random_key().unwrap();
        let recipient = random_key().unwrap();
        let vault_key = random_key().unwrap();
        let ctx_v1 = wrap_context("user_1", 1, "recovery");
        let ctx_v2 = host_wrap_context("user_1", 1, "host_1");

        let wrapped = wrap(&vault_key, &public_of(&recipient), &ctx_v1).unwrap();
        assert_eq!(unwrap(&wrapped, &recipient, &ctx_v1), Some(vault_key));

        let authed = wrap_from(&vault_key, &sender, &public_of(&recipient), &ctx_v2).unwrap();
        let (opened, who) = unwrap_from(&authed, &recipient, &ctx_v2).unwrap();
        assert_eq!(opened, vault_key);
        assert_eq!(who, public_of(&sender));
    }
}
