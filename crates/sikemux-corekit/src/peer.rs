//! Reading a teammate's shared project. Hosts on two accounts never link; instead the asking host
//! shows the answering one a peer ticket, the accounts server's word that it belongs to a member
//! of an org they share, over its own ALPN that carries nothing but these reads.

use serde::{Deserialize, Serialize};
use sikemux_view::task_list::TaskRow;

use crate::accounts::protocol::PeerTicket;
use crate::join::{Refusal, TrustedKeys, CLOCK_SKEW_SECS, MAX_LIFETIME_SECS};

pub const PEER_ALPN: &[u8] = b"sikemux/peer/1";

/// The exact text the server signs.
pub fn ticket_message(ticket: &PeerTicket) -> String {
    format!(
        "sikemux-peer|v1|{}|{}|{}|{}|{}|{}|{}|{}",
        ticket.key_id,
        ticket.org,
        ticket.member,
        ticket.from,
        ticket.to,
        ticket.issued_at,
        ticket.expires_at,
        ticket.member_name
    )
}

/// What the answering host knows that a ticket must agree with.
pub struct Expected<'a> {
    /// The key the asking host proved it holds when it connected.
    pub from: &'a str,
    /// This host's key.
    pub to: &'a str,
    /// Unix seconds.
    pub now: i64,
}

/// Checks the server signed `ticket` for this connection and that it is still good. Whether the
/// org is one this host shares the project with is the caller's to check.
pub fn check(
    ticket: &PeerTicket,
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
    if ticket.to != expected.to {
        return Err(Refusal::WrongHost);
    }
    if ticket.from != expected.from {
        return Err(Refusal::WrongPhone);
    }
    Ok(())
}

/// When a ticket stops being good, in unix seconds, past which the session it opened closes.
pub fn good_until(ticket: &PeerTicket) -> i64 {
    ticket.expires_at + CLOCK_SKEW_SECS
}

/// What the asking host says, one frame at a time on the session's one stream.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PeerSay {
    /// Opens the session, and later keeps it open past the last ticket's end.
    Ticket { ticket: PeerTicket },
    /// Asks for the board of the project known by `project`, the hash of its repository, and
    /// for it again each time it changes.
    Read { project: String },
    /// One question from the asking person's agent about a task on the board being read. The
    /// answer comes back under the same `id`.
    Ask {
        id: u64,
        task: u64,
        question: String,
        /// The asking agent's name.
        agent: String,
        /// The program it runs on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cli: Option<String>,
    },
}

/// What the answering host says back.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PeerHear {
    Board {
        board: SharedBoard,
    },
    /// The project is no longer shared with the ticket's org.
    Gone,
    Refused {
        reason: String,
    },
    Answered {
        id: u64,
        said: serde_json::Value,
    },
    NotAnswered {
        id: u64,
        reason: String,
    },
}

/// A shared project's board as a teammate reads it: no calls, task bodies, chat or rules.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedBoard {
    pub tasks: Vec<TaskRow>,
    pub decisions: Vec<SharedDecision>,
}

/// A decision that stands on a shared project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedDecision {
    pub id: u64,
    pub says: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The agent that made it, by name; none when a person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The person who made it, by first name, when the host knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Unix milliseconds.
    pub at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Vector {
        public: [u8; 32],
        message: String,
        ticket: PeerTicket,
    }

    fn vector() -> Option<Vector> {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../../../server/protocol/vectors/peer.json"))
                .ok()?;
        let text = |name: &str| vector.get(name)?.as_str().map(str::to_owned);
        Some(Vector {
            public: hex::decode(text("publicKey")?).ok()?.try_into().ok()?,
            message: text("message")?,
            ticket: serde_json::from_value(vector.get("ticket")?.clone()).ok()?,
        })
    }

    /// The server's tests sign and check the same vector.
    #[test]
    fn the_shared_peer_vector_checks_out_here_too() -> Result<(), &'static str> {
        let vector = vector().ok_or("the shared peer vector reads")?;
        assert_eq!(ticket_message(&vector.ticket), vector.message);
        let mut keys = TrustedKeys::default();
        keys.insert(&vector.ticket.key_id, vector.public);
        let from = iroh::SecretKey::from_bytes(&[7; 32]).public().to_string();
        let to = iroh::SecretKey::from_bytes(&[8; 32]).public().to_string();
        let expected = |now| Expected {
            from: &from,
            to: &to,
            now,
        };
        let issued = vector.ticket.issued_at;
        assert_eq!(check(&vector.ticket, &keys, &expected(issued + 30)), Ok(()));
        let late = good_until(&vector.ticket) + 1;
        assert_eq!(
            check(&vector.ticket, &keys, &expected(late)),
            Err(Refusal::Expired)
        );
        let mut renamed = vector.ticket.clone();
        renamed.member_name = "Someone Else".into();
        assert_eq!(
            check(&renamed, &keys, &expected(issued)),
            Err(Refusal::BadSignature)
        );
        let swapped = Expected {
            from: &to,
            to: &from,
            now: issued,
        };
        assert_eq!(
            check(&vector.ticket, &keys, &swapped),
            Err(Refusal::WrongHost)
        );
        Ok(())
    }
}
