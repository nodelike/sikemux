//! The recorded team's other hosts, played by the film: it signs their peer tickets with the key
//! the world's core takes for its accounts server's, and asks the core about a task over the peer
//! link as a teammate's core does.

use super::*;
use base64::Engine as _;
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey};
use ring::signature::Ed25519KeyPair;
use sha2::Digest as _;
use sikemux_client::join;
use sikemux_corekit::accounts::protocol::PeerTicket;
use sikemux_corekit::peer::{PeerHear, PeerSay, PEER_ALPN};

pub(super) struct Teammates {
    signer: Ed25519KeyPair,
    /// Each teammate's org and member id, by name, from the recorded account.
    members: Vec<(String, String, String)>,
}

impl Teammates {
    /// Has the core trust the tickets the film signs the way it trusts its accounts server's,
    /// listen for its teammates' hosts, and answer them with the stand-in Claude.
    pub(super) fn seat(work: &Path, home: &Path, claude: &Path) -> anyhow::Result<Self> {
        #[cfg(unix)]
        std::os::unix::fs::symlink(claude, work.join("bin/claude"))?;
        #[cfg(not(unix))]
        let _ = claude;
        let remote = work.join("core.sock.remote.json");
        let mut stored: Value = serde_json::from_slice(&std::fs::read(&remote)?)?;
        (stored.as_object_mut())
            .context("the recorded remote access is not an object")?
            .insert("enabled".into(), json!(true));
        std::fs::write(&remote, serde_json::to_vec(&stored)?)?;
        let pem = super::linked::trust_join_key(home)?;
        let der = (pem.lines())
            .filter(|line| !line.starts_with("-----"))
            .collect::<String>();
        let signer = Ed25519KeyPair::from_pkcs8_maybe_unchecked(
            &base64::engine::general_purpose::STANDARD.decode(der)?,
        )
        .map_err(|_| anyhow!("the join vector's key does not read"))?;
        let shares: Value =
            serde_json::from_slice(&std::fs::read(work.join("data/account/shares.json"))?)?;
        let members = (shares.pointer("/answer/orgs").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .flat_map(|org| {
                let id = (org.get("org").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned();
                (org.get("members").and_then(Value::as_array))
                    .into_iter()
                    .flatten()
                    .filter(|member| member.get("you") != Some(&json!(true)))
                    .filter_map(move |member| {
                        Some((
                            member.get("name")?.as_str()?.to_owned(),
                            id.clone(),
                            member.get("id")?.as_str()?.to_owned(),
                        ))
                    })
            })
            .collect();
        Ok(Self { signer, members })
    }

    /// `name`'s agent `agent` asks the core about task `task` of the project known by `origin`,
    /// and hears what it answers.
    pub(super) async fn ask(
        &self,
        core: &CoreClient,
        origin: &str,
        name: &str,
        agent: &str,
        task: u64,
        question: &str,
    ) -> anyhow::Result<Value> {
        let (_, org, member) = (self.members.iter())
            .find(|(known, _, _)| known == name)
            .with_context(|| format!("the team has no teammate named {name}"))?;
        let status = core.remote_status().await?;
        let host: PublicKey = status.core_id.parse()?;
        let addr = (status.addresses.iter())
            .filter_map(|address| address.parse::<std::net::SocketAddr>().ok())
            .fold(EndpointAddr::new(host), EndpointAddr::with_ip_addr);
        let key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .bind()
            .await?;
        let connection = endpoint.connect(addr, PEER_ALPN).await?;
        let (mut writer, mut reader) = connection.open_bi().await?;
        let now = sikemux_process::clock::epoch_secs() as i64;
        let mut ticket = PeerTicket {
            v: 1,
            key_id: "dev-1".into(),
            org: org.clone(),
            member: member.clone(),
            member_name: name.to_owned(),
            from: key.public().to_string(),
            to: status.core_id,
            issued_at: now,
            expires_at: now + 300,
            signature: String::new(),
        };
        ticket.signature = hex::encode(
            self.signer
                .sign(sikemux_corekit::peer::ticket_message(&ticket).as_bytes()),
        );
        let project = hex::encode(sha2::Sha256::digest(known_by(origin).as_bytes()));
        for say in [
            PeerSay::Ticket { ticket },
            PeerSay::Read { project },
            PeerSay::Ask {
                id: 1,
                task,
                question: question.to_owned(),
                agent: agent.to_owned(),
                cli: Some("claude".into()),
            },
        ] {
            join::send(&mut writer, &say).await?;
        }
        let heard = loop {
            match join::receive::<PeerHear>(&mut reader, Duration::from_secs(120)).await? {
                PeerHear::Board { .. } => {}
                PeerHear::Answered { said, .. } => break Ok(said),
                PeerHear::NotAnswered { reason, .. } | PeerHear::Refused { reason } => {
                    break Err(anyhow!("{name}'s ask was not answered: {reason}"))
                }
                PeerHear::Gone => break Err(anyhow!("the project is not shared with {name}")),
            }
        };
        endpoint.close().await;
        heard
    }
}

/// What the accounts server knows a project by, for the remote the film gives every project.
fn known_by(origin: &str) -> &str {
    let address = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    address.strip_suffix(".git").unwrap_or(address)
}
