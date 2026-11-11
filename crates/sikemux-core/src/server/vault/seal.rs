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

pub(crate) const KEY_LEN: usize = 32;

pub(crate) type Key = [u8; KEY_LEN];

pub(crate) fn random_key() -> Option<Key> {
    let mut key = [0; KEY_LEN];
    SystemRandom::new().fill(&mut key).ok()?;
    Some(key)
}

/// A host's X25519 secret, from its Ed25519 key as signing derives its scalar.
pub(crate) fn host_secret(host: &iroh::SecretKey) -> Key {
    let hashed = Sha512::digest(host.to_bytes());
    let mut scalar = [0; KEY_LEN];
    scalar.copy_from_slice(&hashed[..KEY_LEN]);
    scalar
}

/// A host's X25519 public key, from the Ed25519 key in lowercase hex the account knows it by.
pub(crate) fn host_public(key: &str) -> Option<MontgomeryPoint> {
    let bytes: Key = hex::decode(key).ok()?.try_into().ok()?;
    Some(CompressedEdwardsY(bytes).decompress()?.to_montgomery())
}

pub(crate) fn public_of(secret: &Key) -> MontgomeryPoint {
    MontgomeryPoint::mul_base_clamped(*secret)
}

pub(crate) fn recovery_public(key: &str) -> Option<MontgomeryPoint> {
    let bytes: Key = hex::decode(key).ok()?.try_into().ok()?;
    Some(MontgomeryPoint(bytes))
}

struct Length;

impl hkdf::KeyType for Length {
    fn len(&self) -> usize {
        KEY_LEN
    }
}

fn aead_key(shared: &MontgomeryPoint, context: &str, ephemeral: &[u8]) -> Option<LessSafeKey> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, context.as_bytes());
    let info = [ephemeral];
    let prk = salt.extract(shared.as_bytes());
    let okm = prk.expand(&info, Length).ok()?;
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
pub(crate) fn wrap_context(account: &str, generation: i64, holder: &str) -> String {
    format!("sikemux-vault-key|v1|{account}|{generation}|{holder}")
}

/// The vault key sealed for the holder of `to`'s secret, with a key made for this once.
pub(crate) fn wrap(vault: &Key, to: &MontgomeryPoint, context: &str) -> Option<String> {
    let ephemeral = random_key()?;
    let ephemeral_public = public_of(&ephemeral);
    let shared = to.mul_clamped(ephemeral);
    let key = aead_key(&shared, context, ephemeral_public.as_bytes())?;
    let mut out = ephemeral_public.as_bytes().to_vec();
    out.extend(seal_with(&key, context, vault)?);
    Some(URL_SAFE_NO_PAD.encode(out))
}

pub(crate) fn unwrap(wrapped: &str, secret: &Key, context: &str) -> Option<Key> {
    let bytes = URL_SAFE_NO_PAD.decode(wrapped).ok()?;
    if bytes.len() < KEY_LEN {
        return None;
    }
    let (ephemeral, sealed) = bytes.split_at(KEY_LEN);
    let ephemeral = MontgomeryPoint(ephemeral.try_into().ok()?);
    let shared = ephemeral.mul_clamped(*secret);
    let key = aead_key(&shared, context, ephemeral.as_bytes())?;
    open_with(&key, context, sealed)?.try_into().ok()
}

fn entry_aad(id: &str, generation: i64) -> String {
    format!("sikemux-vault-entry|v1|{id}|{generation}")
}

pub(crate) fn seal_entry(vault: &Key, id: &str, generation: i64, plain: &[u8]) -> Option<String> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, vault).ok()?);
    Some(URL_SAFE_NO_PAD.encode(seal_with(&key, &entry_aad(id, generation), plain)?))
}

pub(crate) fn open_entry(vault: &Key, id: &str, generation: i64, sealed: &str) -> Option<Vec<u8>> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, vault).ok()?);
    let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
    open_with(&key, &entry_aad(id, generation), &bytes)
}

/// Crockford's base32, which leaves out letters that read like digits.
const RECOVERY_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The recovery key as the person writes it down: its 256 bits in base32, four characters a
/// group.
pub(crate) fn recovery_text(secret: &Key) -> String {
    let mut digits = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for byte in secret {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            digits.push(RECOVERY_ALPHABET[((buffer >> bits) & 31) as usize]);
        }
    }
    if bits > 0 {
        digits.push(RECOVERY_ALPHABET[((buffer << (5 - bits)) & 31) as usize]);
    }
    digits
        .chunks(4)
        .map(|group| String::from_utf8_lossy(group).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Reads a recovery key as the person typed it, in any case and with any separators.
pub(crate) fn recovery_secret(typed: &str) -> Option<Key> {
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
