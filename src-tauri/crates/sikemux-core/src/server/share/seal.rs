//! Sealing a share's frames with the key its link holds, so the accounts
//! server that carries them cannot read them.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};

pub(crate) const KEY_LEN: usize = 32;

/// `n` random bytes in unpadded base64url.
pub(crate) fn random_text(n: usize) -> Option<String> {
    let mut bytes = vec![0; n];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) struct Seal {
    key: LessSafeKey,
    /// Binds every frame to its share, so one cannot be passed off as another's.
    aad: Vec<u8>,
}

impl Seal {
    /// `key` is the link's key in unpadded base64url.
    pub(crate) fn new(share: &str, key: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(key).ok()?;
        if bytes.len() != KEY_LEN {
            return None;
        }
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &bytes).ok()?);
        Some(Self {
            key,
            aad: format!("sikemux-share|v1|{share}").into_bytes(),
        })
    }

    /// A random nonce, then the ciphertext and its tag, in unpadded base64url.
    pub(crate) fn seal(&self, plain: &[u8]) -> Option<String> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).ok()?;
        let mut sealed = plain.to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&self.aad),
                &mut sealed,
            )
            .ok()?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        Some(URL_SAFE_NO_PAD.encode(out))
    }

    pub(crate) fn open(&self, sealed: &str) -> Option<Vec<u8>> {
        let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
        if bytes.len() < NONCE_LEN {
            return None;
        }
        let (nonce, rest) = bytes.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut rest = rest.to_vec();
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(&self.aad), &mut rest)
            .ok()?;
        Some(plain.to_vec())
    }
}
