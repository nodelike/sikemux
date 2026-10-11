//! Notifications a host sends a phone through the accounts server. Each is
//! sealed with AES-256-GCM under a key the phone gave this host over their
//! paired connection, so the server, Apple and Google pass it on unread.
//! `server/protocol/vectors/push.json` pins the format for the phone's
//! decryptors.

use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

pub const ENVELOPE_VERSION: u8 = 1;
/// A sealed notification is padded to one of these sizes, so its length says
/// little about what it holds.
pub const PAD_SIZES: [usize; 3] = [512, 1024, 2048];
pub const MAX_PLAINTEXT: usize = 2048;
pub const NONCE_BYTES: usize = 12;
/// What the server accepts in a push's `blob`.
pub const MAX_BLOB_CHARS: usize = 2900;

/// A phone's key for notifications from one host, named by `id` so the phone
/// can tell a notification sealed under a key it has since replaced.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationKey {
    pub id: u32,
    bytes: [u8; 32],
}

impl std::fmt::Debug for NotificationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NotificationKey({:08x})", self.id)
    }
}

impl NotificationKey {
    pub fn new(id: u32, bytes: [u8; 32]) -> Self {
        Self { id, bytes }
    }

    /// `hex` is the 32-byte key as 64 hex characters.
    pub fn from_hex(id: u32, hex: &str) -> Option<Self> {
        let bytes: [u8; 32] = hex::decode(hex).ok()?.try_into().ok()?;
        Some(Self { id, bytes })
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.bytes)
    }

    /// Names one card on the phone without saying what it is about: the
    /// first 16 bytes of HMAC-SHA256 over `sikemux-collapse|<card>`, in hex.
    pub fn collapse_id(&self, card: &str) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &self.bytes);
        let tag = hmac::sign(&key, collapse_text(card).as_bytes());
        hex::encode(tag.as_ref().get(..16).unwrap_or_default())
    }
}

pub fn collapse_text(card: &str) -> String {
    format!("sikemux-collapse|{card}")
}

/// Binds a sealed notification to the host that sent it, the phone it is for
/// and the key it was sealed with, so none of them can be swapped.
pub fn aad(host: &str, phone: &str, key_id: u32) -> String {
    format!("sikemux-push|v1|{host}|{phone}|{key_id:08x}")
}

/// Fills `plaintext` with spaces up to the smallest size that holds it. JSON
/// allows trailing whitespace, so the padded text still parses.
pub fn pad(plaintext: &[u8]) -> Option<Vec<u8>> {
    let size = PAD_SIZES
        .into_iter()
        .find(|size| plaintext.len() <= *size)?;
    let mut padded = Vec::with_capacity(size);
    padded.extend_from_slice(plaintext);
    padded.resize(size, b' ');
    Some(padded)
}

/// `version ‖ keyId (u32, big-endian) ‖ nonce ‖ ciphertext ‖ tag`, in
/// standard base64 with padding. `None` when the plaintext is over
/// [`MAX_PLAINTEXT`].
pub fn seal(key: &NotificationKey, host: &str, phone: &str, plaintext: &[u8]) -> Option<String> {
    let mut nonce = [0u8; NONCE_BYTES];
    SystemRandom::new().fill(&mut nonce).ok()?;
    seal_with_nonce(key, host, phone, plaintext, nonce)
}

pub fn seal_with_nonce(
    key: &NotificationKey,
    host: &str,
    phone: &str,
    plaintext: &[u8],
    nonce: [u8; NONCE_BYTES],
) -> Option<String> {
    let mut sealed = pad(plaintext)?;
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key.bytes).ok()?);
    let aad = aad(host, phone, key.id);
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_bytes()),
            &mut sealed,
        )
        .ok()?;
    let mut blob = Vec::with_capacity(1 + 4 + NONCE_BYTES + sealed.len());
    blob.push(ENVELOPE_VERSION);
    blob.extend_from_slice(&key.id.to_be_bytes());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&sealed);
    Some(base64::engine::general_purpose::STANDARD.encode(blob))
}

/// What the phone does: the plaintext with its padding trimmed, or `None`
/// when the blob was not sealed for this host, phone and key.
pub fn open(key: &NotificationKey, host: &str, phone: &str, blob: &str) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(blob)
        .ok()?;
    let (&version, rest) = bytes.split_first()?;
    let (key_id, rest) = rest.split_first_chunk::<4>()?;
    let (nonce, sealed) = rest.split_first_chunk::<NONCE_BYTES>()?;
    if version != ENVELOPE_VERSION || u32::from_be_bytes(*key_id) != key.id {
        return None;
    }
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key.bytes).ok()?);
    let aad = aad(host, phone, key.id);
    let mut sealed = sealed.to_vec();
    let opened = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(aad.as_bytes()),
            &mut sealed,
        )
        .ok()?;
    let end = opened
        .iter()
        .rposition(|byte| *byte != b' ')
        .map_or(0, |last| last + 1);
    Some(opened.get(..end)?.to_vec())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationKind {
    /// A chat agent asks to do something; the card may carry Allow and Reject.
    Permission,
    /// A terminal agent waits for the person to type.
    Input,
    /// A turn that ran a while has ended.
    Finished,
    /// An agent reported an error or stopped unexpectedly.
    Problem,
    /// Removes the card with the same `collapseId`; shows nothing.
    Clear,
}

/// The Android channel a card goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationChannel {
    NeedsYou,
    Finished,
    Problems,
}

/// The iOS category, which decides the card's buttons. Only `permission`
/// has any: Reject and Allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationCategory {
    Permission,
    NeedsYou,
    Finished,
    Problem,
}

impl NotificationKind {
    pub fn channel(self) -> Option<NotificationChannel> {
        match self {
            Self::Permission | Self::Input => Some(NotificationChannel::NeedsYou),
            Self::Finished => Some(NotificationChannel::Finished),
            Self::Problem => Some(NotificationChannel::Problems),
            Self::Clear => None,
        }
    }
}

/// What the phone reads once it opens a sealed notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub v: u8,
    pub kind: NotificationKind,
    /// `None` for `clear`.
    pub channel: Option<NotificationChannel>,
    /// `None` for `clear`. `permission` only when both answers are offered.
    pub category: Option<NotificationCategory>,
    /// The same id the push carried, for removing this card later.
    pub collapse_id: String,
    /// Groups one agent's cards: `<hostKey>/<agentId>`.
    pub thread: String,
    pub host_key: String,
    pub host_name: String,
    pub agent_id: String,
    pub provider: String,
    pub chat_title: Option<String>,
    pub title: String,
    pub body: String,
    /// One line to show in monospace, such as the command an agent wants to run.
    pub detail: Option<String>,
    /// Where tapping the card goes.
    pub url: String,
    /// The permission request to answer with `answer_permission`.
    pub request_id: Option<String>,
    pub allow_option_id: Option<String>,
    pub reject_option_id: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub expires_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/push.json"
        ))
        .unwrap()
    }

    fn text(vector: &serde_json::Value, name: &str) -> String {
        vector[name].as_str().unwrap().to_owned()
    }

    fn vector_key(vector: &serde_json::Value) -> NotificationKey {
        let id = u32::from_str_radix(&text(vector, "keyId"), 16).unwrap();
        NotificationKey::from_hex(id, &text(vector, "key")).unwrap()
    }

    /// The same vector the phone's decryptors check, so both sides agree on
    /// every byte.
    #[test]
    fn notifications_seal_to_the_shared_vector() {
        let vector = vector();
        let key = vector_key(&vector);
        let (host, phone) = (text(&vector, "hostKey"), text(&vector, "phoneKey"));
        assert_eq!(aad(&host, &phone, key.id), text(&vector, "aad"));
        assert_eq!(
            iroh::SecretKey::from_bytes(&[9; 32]).public().to_string(),
            phone
        );

        let notification: Notification =
            serde_json::from_value(vector["notification"].clone()).unwrap();
        let plaintext = serde_json::to_string(&notification).unwrap();
        assert_eq!(plaintext, text(&vector, "plaintext"));
        assert_eq!(
            pad(plaintext.as_bytes()).unwrap().len() as u64,
            vector["paddedLength"].as_u64().unwrap()
        );

        let nonce: [u8; NONCE_BYTES] = hex::decode(text(&vector, "nonce"))
            .unwrap()
            .try_into()
            .unwrap();
        let blob = seal_with_nonce(&key, &host, &phone, plaintext.as_bytes(), nonce).unwrap();
        assert_eq!(blob, text(&vector, "blob"));
        assert_eq!(
            open(&key, &host, &phone, &blob).unwrap(),
            plaintext.as_bytes()
        );

        assert_eq!(
            collapse_text(&text(&vector, "card")),
            text(&vector, "collapseText")
        );
        assert_eq!(
            key.collapse_id(&text(&vector, "card")),
            text(&vector, "collapseId")
        );
        assert_eq!(notification.collapse_id, text(&vector, "collapseId"));
    }

    #[test]
    fn a_blob_opens_only_for_its_own_host_phone_and_key() {
        let vector = vector();
        let key = vector_key(&vector);
        let (host, phone) = (text(&vector, "hostKey"), text(&vector, "phoneKey"));
        let blob = text(&vector, "blob");
        assert!(open(&key, &host, &phone, &blob).is_some());
        assert!(open(&key, &phone, &host, &blob).is_none());
        assert!(open(&key, &host, &"0".repeat(64), &blob).is_none());
        let renamed = NotificationKey::new(key.id + 1, key.bytes);
        assert!(open(&renamed, &host, &phone, &blob).is_none());
        let other = NotificationKey::new(key.id, [9; 32]);
        assert!(open(&other, &host, &phone, &blob).is_none());
    }

    #[test]
    fn plaintext_is_padded_to_the_smallest_size_that_holds_it() {
        assert_eq!(pad(b"{}").unwrap().len(), 512);
        assert_eq!(pad(&[b'x'; 512]).unwrap().len(), 512);
        assert_eq!(pad(&[b'x'; 513]).unwrap().len(), 1024);
        assert_eq!(pad(&[b'x'; 1025]).unwrap().len(), 2048);
        assert!(pad(&[b'x'; 2049]).is_none());
        let padded = pad(br#"{"v":1}"#).unwrap();
        assert!(padded.ends_with(b"   "));
        let parsed: serde_json::Value = serde_json::from_slice(&padded).unwrap();
        assert_eq!(parsed["v"], 1);
    }

    #[test]
    fn the_largest_notification_fits_the_server_limit() {
        let key = NotificationKey::new(1, [3; 32]);
        let host = "a".repeat(64);
        let blob = seal(&key, &host, &host, &[b'x'; MAX_PLAINTEXT]).unwrap();
        assert!(blob.len() <= MAX_BLOB_CHARS, "{} chars", blob.len());
        let first = seal(&key, &host, &host, b"{}").unwrap();
        let second = seal(&key, &host, &host, b"{}").unwrap();
        assert_ne!(first, second, "every seal takes a fresh nonce");
    }
}
