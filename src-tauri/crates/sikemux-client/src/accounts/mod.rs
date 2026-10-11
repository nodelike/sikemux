//! What a client signs for the accounts server at api.sikemux.com, how it
//! reaches the server, and which relays it meets hosts through.

pub mod network;

pub use sikemux_wire::accounts::protocol;

/// TLS for the accounts API, trusting the usual web roots.
pub fn tls_config() -> Option<std::sync::Arc<tokio_rustls::rustls::ClientConfig>> {
    use std::sync::{Arc, OnceLock};
    use tokio_rustls::rustls;
    static CONFIG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let builder = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .ok()?;
            Some(Arc::new(
                builder.with_root_certificates(roots).with_no_client_auth(),
            ))
        })
        .clone()
}

/// What a device signs to register with an account. It binds the server's
/// one-time challenge, the account and the key, so the signature proves
/// nothing else. The server checks exactly this text.
pub fn registration_message(nonce: &str, user_id: &str, key: &str) -> String {
    format!("sikemux-register|{nonce}|{user_id}|{key}")
}

/// Accepts only a challenge and a Clerk user id, so a request to sign a
/// registration cannot make the core sign any other text.
pub fn check_registration(nonce: &str, user_id: &str) -> Result<(), &'static str> {
    check_live(nonce)?;
    check_user_id(user_id)
}

/// What a device signs to open its live connection to the account. The prefix
/// differs from registration's, so neither signature stands in for the other.
pub fn live_message(nonce: &str, key: &str) -> String {
    format!("sikemux-live|{nonce}|{key}")
}

/// Accepts only a server challenge: 64 lowercase hex characters.
pub fn check_live(nonce: &str) -> Result<(), &'static str> {
    let is_challenge = nonce.len() == 64
        && nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if is_challenge {
        Ok(())
    } else {
        Err("the challenge is 64 lowercase hex characters")
    }
}

/// Clerk user ids look like `user_2abcXYZ`.
pub fn check_user_id(user_id: &str) -> Result<(), &'static str> {
    let valid = user_id.strip_prefix("user_").is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 64 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
    });
    if valid {
        Ok(())
    } else {
        Err("the account is a Clerk user id like user_2abc")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registration_text_is_only_ever_a_challenge_and_an_account() {
        let nonce = "a".repeat(64);
        assert!(super::check_registration(&nonce, "user_2abc").is_ok());
        assert!(super::check_registration("short", "user_2abc").is_err());
        assert!(super::check_registration(&"A".repeat(64), "user_2abc").is_err());
        assert!(super::check_registration(&nonce, "user_").is_err());
        assert!(super::check_registration(&nonce, "org_2abc").is_err());
        assert!(super::check_registration(&nonce, "user_2abc|anything").is_err());
    }

    #[test]
    fn live_hellos_sign_the_text_the_server_checks() -> Result<(), Box<dyn std::error::Error>> {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../server/protocol/vectors/live.json"
        ))?;
        let text = |name: &str| vector[name].as_str().unwrap_or_default().to_owned();
        let bytes: [u8; 32] = hex::decode(text("secretKey"))?
            .try_into()
            .map_err(|_| "the secret is 32 bytes")?;
        let key = iroh::SecretKey::from_bytes(&bytes);
        assert_eq!(key.public().to_string(), text("key"));
        let message = super::live_message(&text("nonce"), &text("key"));
        assert_eq!(message, text("message"));
        assert_eq!(
            hex::encode(key.sign(message.as_bytes()).to_bytes()),
            text("signature")
        );
        Ok(())
    }

    #[test]
    fn live_text_is_only_ever_a_challenge() {
        assert!(super::check_live(&"a".repeat(64)).is_ok());
        assert!(super::check_live("short").is_err());
        assert!(super::check_live(&"A".repeat(64)).is_err());
        assert!(super::check_live(&format!("{}|x", "a".repeat(62))).is_err());
    }
}
