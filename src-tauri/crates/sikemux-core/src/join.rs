//! Joining a host. A phone signed in to the same account as the host gets a
//! short-lived ticket from the accounts server naming the account, the host
//! and the phone, and hands it to the host. The host checks the ticket itself,
//! never calling the server, and then asks the person whether to allow the
//! phone: the ticket only gets the phone as far as the question.

use std::collections::BTreeMap;

pub use sikemux_client::join::*;

use crate::accounts::protocol::JoinTicket;

/// How far the host's clock may disagree with the server's.
pub const CLOCK_SKEW_SECS: i64 = 60;
pub const MAX_LIFETIME_SECS: i64 = 600;
/// How long after a revocation a ticket issued before it could still arrive
/// within its life.
pub const REVOCATION_MEMORY_MS: u64 = ((MAX_LIFETIME_SECS + 2 * CLOCK_SKEW_SECS) * 1000) as u64;

const PRODUCTION_KEYS: &[(&str, &str)] = &[(
    "prod-1",
    "7f72791233bdb262c930822cce460eb36483842ae8053f44193a4b3e3dc9ff6c",
)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Unreadable,
    UnsupportedVersion,
    UnknownKey,
    BadSignature,
    NotYetValid,
    Expired,
    TooLong,
    SignedOut,
    WrongAccount,
    WrongHost,
    WrongPhone,
    Revoked,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnknownKey => "unknown_key",
            Self::BadSignature => "bad_signature",
            Self::NotYetValid => "not_yet_valid",
            Self::Expired => "expired",
            Self::TooLong => "too_long",
            Self::SignedOut => "signed_out",
            Self::WrongAccount => "wrong_account",
            Self::WrongHost => "wrong_host",
            Self::WrongPhone => "wrong_phone",
            Self::Revoked => "revoked",
        }
    }
}

/// The exact text the server signs.
pub fn ticket_message(ticket: &JoinTicket) -> String {
    format!(
        "sikemux-join|v1|{}|{}|{}|{}|{}|{}",
        ticket.key_id,
        ticket.account,
        ticket.host,
        ticket.phone,
        ticket.issued_at,
        ticket.expires_at
    )
}

/// The server keys a host trusts to sign tickets, by key id.
#[derive(Clone, Debug, Default)]
pub struct TrustedKeys(BTreeMap<String, [u8; 32]>);

impl TrustedKeys {
    pub fn production() -> Self {
        let mut keys = Self::default();
        for (id, key) in PRODUCTION_KEYS {
            let key = hex::decode(key)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .expect("a production join key is 32 bytes of hex");
            keys.insert(id, key);
        }
        keys
    }

    /// Production's keys, and in a dev build the local accounts server's key
    /// as well. Release builds never read a key from disk.
    pub fn for_this_build() -> Self {
        #[allow(unused_mut)]
        let mut keys = Self::production();
        #[cfg(debug_assertions)]
        if let Some(key) = dev_key() {
            keys.insert("dev-1", key);
        }
        keys
    }

    pub fn insert(&mut self, id: &str, key: [u8; 32]) {
        self.0.insert(id.to_owned(), key);
    }

    fn get(&self, id: &str) -> Option<&[u8; 32]> {
        self.0.get(id)
    }
}

/// The dev API's signing key, which it writes on first start.
#[cfg(debug_assertions)]
fn dev_key() -> Option<[u8; 32]> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    let path = std::path::Path::new(&home).join(".config/sikemux/dev/join-signing-key.pem");
    public_key_from_pem(&std::fs::read_to_string(path).ok()?)
}

/// The public half of a PKCS #8 Ed25519 private key in PEM.
#[cfg(any(debug_assertions, test))]
fn public_key_from_pem(pem: &str) -> Option<[u8; 32]> {
    use base64::Engine;
    use ring::signature::KeyPair;
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).ok()?;
    pair.public_key().as_ref().try_into().ok()
}

/// What the host knows that a ticket must agree with.
pub struct Expected<'a> {
    /// The account the host is signed in to.
    pub owner: Option<&'a str>,
    pub host: &'a str,
    /// The key the phone proved it holds when it connected.
    pub phone: &'a str,
    /// Unix seconds.
    pub now: i64,
    /// When the account last revoked the phone, in unix milliseconds.
    pub revoked_at_ms: Option<u64>,
}

pub fn check(
    ticket: &JoinTicket,
    keys: &TrustedKeys,
    expected: &Expected<'_>,
) -> Result<(), Refusal> {
    if ticket.v != 1 {
        return Err(Refusal::UnsupportedVersion);
    }
    let key = keys.get(&ticket.key_id).ok_or(Refusal::UnknownKey)?;
    let signature = hex::decode(&ticket.signature).map_err(|_| Refusal::BadSignature)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(ticket_message(ticket).as_bytes(), &signature)
        .map_err(|_| Refusal::BadSignature)?;
    let life = ticket.expires_at - ticket.issued_at;
    if !(0..=MAX_LIFETIME_SECS).contains(&life) {
        return Err(Refusal::TooLong);
    }
    if expected.now < ticket.issued_at - CLOCK_SKEW_SECS {
        return Err(Refusal::NotYetValid);
    }
    if expected.now > ticket.expires_at + CLOCK_SKEW_SECS {
        return Err(Refusal::Expired);
    }
    let owner = expected.owner.ok_or(Refusal::SignedOut)?;
    if ticket.account != owner {
        return Err(Refusal::WrongAccount);
    }
    if ticket.host != expected.host {
        return Err(Refusal::WrongHost);
    }
    if ticket.phone != expected.phone {
        return Err(Refusal::WrongPhone);
    }
    let issued_ms = u64::try_from(ticket.issued_at)
        .unwrap_or(0)
        .saturating_mul(1000);
    if expected
        .revoked_at_ms
        .is_some_and(|revoked| revoked >= issued_ms)
    {
        return Err(Refusal::Revoked);
    }
    Ok(())
}

/// The shared vector the server's tests check too, and a signer with its key.
#[cfg(test)]
pub(crate) mod vector {
    use ring::signature::Ed25519KeyPair;

    use super::*;

    pub(crate) struct Vector {
        signer: Ed25519KeyPair,
        pub public: [u8; 32],
        pub pem: String,
        pub message: String,
        pub ticket: JoinTicket,
    }

    pub(crate) fn vector() -> Vector {
        let vector: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../server/protocol/vectors/join.json"
        ))
        .unwrap();
        let text = |name: &str| vector[name].as_str().unwrap().to_owned();
        let seed = hex::decode(text("secretKey")).unwrap();
        Vector {
            signer: Ed25519KeyPair::from_seed_unchecked(&seed).unwrap(),
            public: hex::decode(text("publicKey")).unwrap().try_into().unwrap(),
            pem: text("privateKeyPem"),
            message: text("message"),
            ticket: serde_json::from_value(vector["ticket"].clone()).unwrap(),
        }
    }

    impl Vector {
        pub fn keys(&self) -> TrustedKeys {
            let mut keys = TrustedKeys::default();
            keys.insert(&self.ticket.key_id, self.public);
            keys
        }

        pub fn signed(&self, edit: impl FnOnce(&mut JoinTicket)) -> JoinTicket {
            let mut ticket = self.ticket.clone();
            edit(&mut ticket);
            ticket.signature = hex::encode(self.signer.sign(ticket_message(&ticket).as_bytes()));
            ticket
        }

        pub fn check<'a>(
            &'a self,
            ticket: &JoinTicket,
            edit: impl FnOnce(&mut Expected<'a>),
        ) -> Result<(), Refusal> {
            let mut expected = Expected {
                owner: Some(&self.ticket.account),
                host: &self.ticket.host,
                phone: &self.ticket.phone,
                now: self.ticket.issued_at + 30,
                revoked_at_ms: None,
            };
            edit(&mut expected);
            check(ticket, &self.keys(), &expected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vector::vector;
    use super::*;
    use crate::protocol::DeviceAccess;

    #[test]
    fn the_shared_vector_signs_the_text_the_server_signs_and_checks_out() {
        let vector = vector();
        assert_eq!(ticket_message(&vector.ticket), vector.message);
        assert_eq!(public_key_from_pem(&vector.pem), Some(vector.public));
        assert_eq!(vector.signed(|_| {}), vector.ticket);
        assert_eq!(vector.check(&vector.ticket, |_| {}), Ok(()));
        let host = iroh::SecretKey::from_bytes(&[8; 32]).public().to_string();
        let phone = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        assert_eq!(
            (host, phone),
            (vector.ticket.host.clone(), vector.ticket.phone.clone())
        );
    }

    #[test]
    fn a_ticket_another_key_signed_or_someone_edited_is_refused() {
        let vector = vector();
        let mut edited = vector.ticket.clone();
        edited.expires_at += 60;
        assert_eq!(vector.check(&edited, |_| {}), Err(Refusal::BadSignature));
        let mut unknown = vector.ticket.clone();
        unknown.key_id = "prod-9".into();
        assert_eq!(vector.check(&unknown, |_| {}), Err(Refusal::UnknownKey));
        let mut garbled = vector.ticket.clone();
        garbled.signature = "zz".into();
        assert_eq!(vector.check(&garbled, |_| {}), Err(Refusal::BadSignature));
        let later = vector.signed(|ticket| ticket.v = 2);
        assert_eq!(
            vector.check(&later, |_| {}),
            Err(Refusal::UnsupportedVersion)
        );
    }

    #[test]
    fn a_ticket_is_good_only_within_its_life_give_or_take_a_minute() {
        let vector = vector();
        let ticket = &vector.ticket;
        let at = |now: i64| vector.check(ticket, |expected| expected.now = now);
        assert_eq!(at(ticket.issued_at - 60), Ok(()));
        assert_eq!(at(ticket.issued_at - 61), Err(Refusal::NotYetValid));
        assert_eq!(at(ticket.expires_at + 60), Ok(()));
        assert_eq!(at(ticket.expires_at + 61), Err(Refusal::Expired));
        let long = vector.signed(|ticket| ticket.expires_at = ticket.issued_at + 601);
        assert_eq!(vector.check(&long, |_| {}), Err(Refusal::TooLong));
        let backwards = vector.signed(|ticket| ticket.expires_at = ticket.issued_at - 1);
        assert_eq!(vector.check(&backwards, |_| {}), Err(Refusal::TooLong));
    }

    #[test]
    fn a_ticket_must_name_this_account_this_host_and_the_phone_holding_it() {
        let vector = vector();
        let ticket = &vector.ticket;
        let stranger = iroh::SecretKey::generate().public().to_string();
        assert_eq!(
            vector.check(ticket, |expected| expected.owner = None),
            Err(Refusal::SignedOut)
        );
        assert_eq!(
            vector.check(ticket, |expected| expected.owner =
                Some("user_2someoneElse")),
            Err(Refusal::WrongAccount)
        );
        assert_eq!(
            vector.check(ticket, |expected| expected.host = &stranger),
            Err(Refusal::WrongHost)
        );
        assert_eq!(
            vector.check(ticket, |expected| expected.phone = &stranger),
            Err(Refusal::WrongPhone)
        );
    }

    #[test]
    fn a_ticket_issued_before_the_phone_was_revoked_is_refused() {
        let vector = vector();
        let ticket = &vector.ticket;
        let issued_ms = ticket.issued_at as u64 * 1000;
        let revoked = |at: u64| vector.check(ticket, |expected| expected.revoked_at_ms = Some(at));
        assert_eq!(revoked(issued_ms + 5_000), Err(Refusal::Revoked));
        assert_eq!(revoked(issued_ms), Err(Refusal::Revoked));
        assert_eq!(revoked(issued_ms - 1), Ok(()));
    }

    #[test]
    fn production_trusts_only_its_own_key() {
        let keys = TrustedKeys::production();
        assert_eq!(keys.0.keys().collect::<Vec<_>>(), vec!["prod-1"]);
    }

    #[test]
    fn the_phone_may_say_its_name_beside_the_ticket() {
        let vector = vector();
        let hello = JoinHello {
            ticket: vector.ticket.clone(),
            name: "Pixel 8".into(),
            platform: "android".into(),
        };
        let text = serde_json::to_value(&hello).unwrap();
        assert_eq!(text["keyId"], "vector-1");
        assert_eq!(text["name"], "Pixel 8");
        let bare: JoinHello =
            serde_json::from_value(serde_json::to_value(&vector.ticket).unwrap()).unwrap();
        assert_eq!(bare.ticket, vector.ticket);
        assert_eq!(bare.name, "");
    }

    #[test]
    fn replies_read_as_the_contract_spells_them() {
        let text = |reply: JoinReply| serde_json::to_string(&reply).unwrap();
        assert_eq!(
            text(JoinReply::Allowed {
                access: DeviceAccess::Watch
            }),
            r#"{"result":"allowed","access":"watch"}"#
        );
        assert_eq!(text(JoinReply::Denied), r#"{"result":"denied"}"#);
        assert_eq!(
            text(JoinReply::Refused {
                reason: Refusal::WrongHost.reason().into()
            }),
            r#"{"result":"refused","reason":"wrong_host"}"#
        );
    }
}
