use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use sikemux_calls::vault::VaultFieldKind;
use url::Url;

pub(crate) const KEY_LEN: usize = 32;
pub(crate) type Key = [u8; KEY_LEN];

#[cfg(not(test))]
const MASTER_KEY_SERVICE: &str = if cfg!(debug_assertions) {
    "sikemux-agent-vault-dev"
} else {
    "sikemux-agent-vault"
};
#[cfg(test)]
const MASTER_KEY_SERVICE: &str = "sikemux-agent-vault-test";
const MASTER_KEY_ACCOUNT: &str = "local-master-v1";

pub(crate) fn random_key() -> Result<Key, String> {
    let mut key = [0u8; KEY_LEN];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| "system randomness unavailable".to_owned())?;
    Ok(key)
}

pub(crate) fn load_or_create_master_key() -> Result<Key, String> {
    if let Some(encoded) = sikemux_keychain::read(MASTER_KEY_SERVICE, MASTER_KEY_ACCOUNT)
        .map_err(|error| format!("could not read Agentic Vault key from keychain: {error}"))?
    {
        let decoded = URL_SAFE_NO_PAD
            .decode(encoded.trim())
            .map_err(|_| "stored Agentic Vault key is corrupted".to_owned())?;
        let key: Key = decoded
            .try_into()
            .map_err(|_| "stored Agentic Vault key has invalid length".to_owned())?;
        return Ok(key);
    }
    let key = random_key()?;
    let encoded = URL_SAFE_NO_PAD.encode(key);
    sikemux_keychain::write(MASTER_KEY_SERVICE, MASTER_KEY_ACCOUNT, &encoded)
        .map_err(|error| format!("could not save Agentic Vault key to keychain: {error}"))?;
    Ok(key)
}

pub(crate) fn seal(key: &Key, aad: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    let unbound = UnboundKey::new(&AES_256_GCM, key)
        .map_err(|_| "invalid vault encryption key".to_owned())?;
    let aead = LessSafeKey::new(unbound);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "system randomness unavailable".to_owned())?;
    let mut buf = plain.to_vec();
    aead.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad.as_bytes()),
        &mut buf,
    )
    .map_err(|_| "failed to seal vault payload".to_owned())?;
    let mut out = Vec::with_capacity(NONCE_LEN + buf.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buf);
    Ok(out)
}

pub(crate) fn open(key: &Key, aad: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < NONCE_LEN {
        return Err("sealed vault payload is truncated".to_owned());
    }
    let (nonce_bytes, cipher) = sealed.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce_bytes)
        .map_err(|_| "invalid vault payload nonce".to_owned())?;
    let unbound = UnboundKey::new(&AES_256_GCM, key)
        .map_err(|_| "invalid vault encryption key".to_owned())?;
    let aead = LessSafeKey::new(unbound);
    let mut buf = cipher.to_vec();
    let plain = aead
        .open_in_place(nonce, Aad::from(aad.as_bytes()), &mut buf)
        .map_err(|_| "could not decrypt vault payload (wrong key or tampered data)".to_owned())?;
    Ok(plain.to_vec())
}

pub(crate) fn sha256_prefix(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hexed = hex::encode(digest);
    hexed.chars().take(12).collect()
}

pub(crate) fn valid_entry_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedVaultRef {
    pub(crate) entry_id: String,
    pub(crate) field_key: Option<String>,
}

pub(crate) fn parse_vault_ref(raw: &str) -> Option<ParsedVaultRef> {
    let trimmed = raw.trim();
    let body = trimmed.strip_prefix("vault:")?;
    if body.is_empty() {
        return None;
    }
    match body.split_once('.') {
        Some((entry_id, field_key)) => {
            let entry_id = entry_id.trim();
            let field_key = field_key.trim();
            if !valid_entry_id(entry_id) || field_key.is_empty() || field_key.len() > 128 {
                return None;
            }
            if !field_key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                return None;
            }
            Some(ParsedVaultRef {
                entry_id: entry_id.to_owned(),
                field_key: Some(field_key.to_owned()),
            })
        }
        None => {
            let entry_id = body.trim();
            if !valid_entry_id(entry_id) {
                return None;
            }
            Some(ParsedVaultRef {
                entry_id: entry_id.to_owned(),
                field_key: None,
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedOpRef {
    pub(crate) vault: String,
    pub(crate) item: String,
    pub(crate) section: Option<String>,
    pub(crate) field: String,
}

pub(crate) fn parse_op_ref(raw: &str) -> Result<ParsedOpRef, String> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("op://")
        .ok_or_else(|| "1Password reference must start with op://".to_owned())?;
    if body.chars().any(char::is_control) {
        return Err("1Password reference cannot contain control characters".to_owned());
    }
    let parts: Vec<&str> = body.split('/').collect();
    let non_empty = |segment: &str| !segment.trim().is_empty();
    match parts.as_slice() {
        [vault, item, field] if non_empty(vault) && non_empty(item) && non_empty(field) => {
            Ok(ParsedOpRef {
                vault: (*vault).to_owned(),
                item: (*item).to_owned(),
                section: None,
                field: (*field).to_owned(),
            })
        }
        [vault, item, section, field]
            if non_empty(vault)
                && non_empty(item)
                && non_empty(section)
                && non_empty(field) =>
        {
            Ok(ParsedOpRef {
                vault: (*vault).to_owned(),
                item: (*item).to_owned(),
                section: Some((*section).to_owned()),
                field: (*field).to_owned(),
            })
        }
        _ => Err(
            "1Password reference must follow op://<vault>/<item>/<field> or op://<vault>/<item>/<section>/<field>"
                .to_owned(),
        ),
    }
}

pub(crate) fn normalize_domain(raw: &str) -> String {
    let trimmed = raw.trim().to_lowercase();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Ok(parsed) = Url::parse(&trimmed) {
        if let Some(host) = parsed.host_str() {
            return host.trim_end_matches('.').to_owned();
        }
    }
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(&trimmed);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = host_port
        .split(':')
        .next()
        .unwrap_or(host_port)
        .trim()
        .trim_end_matches('.');
    host.to_owned()
}

pub(crate) fn domain_matches(pattern: &str, domain: &str) -> bool {
    let pattern = normalize_domain(pattern);
    let domain = normalize_domain(domain);
    if pattern.is_empty() || domain.is_empty() {
        return false;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        domain == suffix || domain.ends_with(&format!(".{suffix}"))
    } else {
        domain == pattern
    }
}

pub(crate) fn mask_value(kind: VaultFieldKind, sensitive: bool, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if kind == VaultFieldKind::Totp {
        return "6 digits · live".to_owned();
    }
    if !sensitive && !kind.is_inherently_sensitive() {
        return trimmed.to_owned();
    }
    match kind {
        VaultFieldKind::CardNumber => {
            let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
            let last4: String = digits
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if last4.len() == 4 {
                format!("•••• •••• •••• {last4}")
            } else {
                "••••••••••••".to_owned()
            }
        }
        VaultFieldKind::Email => match trimmed.split_once('@') {
            Some((local, domain)) => {
                let first = local.chars().next().unwrap_or('•');
                format!("{first}•••@{domain}")
            }
            None => "••••••••".to_owned(),
        },
        VaultFieldKind::TaxId | VaultFieldKind::AccountNumber => {
            let tail: String = trimmed
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if trimmed.chars().count() > 4 {
                format!("•••••{tail}")
            } else {
                "••••".to_owned()
            }
        }
        _ => "••••••••".to_owned(),
    }
}

pub(crate) fn redaction_tokens(value: &str, kind: VaultFieldKind) -> Vec<String> {
    let mut tokens = Vec::new();
    let trimmed = value.trim();
    if trimmed.chars().count() >= 3 {
        tokens.push(trimmed.to_owned());
    }
    if matches!(
        kind,
        VaultFieldKind::CardNumber | VaultFieldKind::AccountNumber | VaultFieldKind::Phone
    ) {
        let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
        if digits.len() >= 6 && digits != trimmed {
            tokens.push(digits.clone());
        }
        if kind == VaultFieldKind::CardNumber && digits.len() >= 12 {
            let spaced: String = digits
                .as_bytes()
                .chunks(4)
                .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            if !tokens.contains(&spaced) {
                tokens.push(spaced);
            }
        }
    }
    tokens.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    tokens.dedup();
    tokens
}

pub(crate) fn generate_totp(raw_secret: &str, epoch_secs: u64) -> Result<String, String> {
    let secret = extract_totp_secret(raw_secret);
    let key_bytes = decode_base32(&secret)?;
    if key_bytes.is_empty() {
        return Err("TOTP secret is empty".to_owned());
    }
    let counter = epoch_secs / 30;
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &key_bytes);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let digest = tag.as_ref();
    let offset = usize::from(digest.last().copied().unwrap_or(0) & 0x0f);
    let b0 = u32::from(digest.get(offset).copied().unwrap_or(0) & 0x7f);
    let b1 = u32::from(digest.get(offset + 1).copied().unwrap_or(0));
    let b2 = u32::from(digest.get(offset + 2).copied().unwrap_or(0));
    let b3 = u32::from(digest.get(offset + 3).copied().unwrap_or(0));
    let code = ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) % 1_000_000;
    Ok(format!("{code:06}"))
}

fn extract_totp_secret(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("otpauth://") {
        if let Ok(url) = Url::parse(trimmed) {
            if let Some((_, secret)) = url.query_pairs().find(|(k, _)| k == "secret") {
                return secret.into_owned();
            }
        }
    }
    trimmed.to_owned()
}

fn decode_base32(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '=' {
            continue;
        }
        let val = match ch.to_ascii_uppercase() {
            'A'..='Z' => (ch.to_ascii_uppercase() as u32) - ('A' as u32),
            '2'..='7' => (ch as u32) - ('2' as u32) + 26,
            _ => return Err("TOTP secret must be valid Base32".to_owned()),
        };
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}
