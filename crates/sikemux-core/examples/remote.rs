//! Talks to a core from another machine, the way the phone app does, over the
//! real network.
//!
//! As the Mac, against a core's socket:
//!   remote mac <socket> on | off | allow | status
//!   remote mac <socket> spawn
//!   remote mac <socket> publish            offers the fake agent to devices
//!   remote mac <socket> chat [prompt]      starts a chat with it
//!   remote mac <socket> say <agent> <text>
//!   remote mac <socket> backdrop on|off [image.jpg]  publishes the pane backdrop
//!   remote mac <socket> chats              lists the chats running on the core
//!   remote mac <socket> sleepy             keeps a sleeping chat for a device to wake
//! As a device, keeping its key in `<key-file>`:
//!   remote device <key-file> sessions <core-id>

use sikemux_wire::protocol::ask;
use std::path::Path;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use sikemux_client::accounts::network;
use sikemux_client::client::CoreClient;
use sikemux_client::remote::{self, SecretKey};
use sikemux_wire::protocol::{DeviceAccess, LaunchIdentity, SpawnTarget, TerminalSpawn};

type Failure = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["mac", socket, "spawn"] => spawn(Path::new(socket)).await,
        ["mac", socket, "publish"] => publish(Path::new(socket)).await,
        ["mac", socket, "chat", prompt @ ..] => chat(Path::new(socket), &prompt.join(" ")).await,
        ["mac", socket, "say", agent, text @ ..] => say(Path::new(socket), agent, &text.join(" ")).await,
        ["mac", socket, "sleepy"] => sleepy(Path::new(socket)).await,
        ["mac", socket, "chats"] => chats(Path::new(socket)).await,
        ["mac", socket, action] => mac(Path::new(socket), action).await,
        ["device", key, "sessions", core] => sessions(Path::new(key), core).await,
        _ => Err("usage: remote mac <socket> on|off|allow|status|spawn|publish|chat|say | remote device <key-file> sessions <core-id>".into()),
    }
}

async fn mac(socket: &Path, action: &str) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let status = match action {
        "on" => client.set_remote_access(true).await?,
        "off" => client.set_remote_access(false).await?,
        "allow" => {
            let status = client.remote_status().await?;
            let waiting = status.pending.first().ok_or("no device is waiting")?;
            client
                .answer_pairing(waiting.id.clone(), true, DeviceAccess::Full)
                .await?
        }
        "status" => client.remote_status().await?,
        other => return Err(format!("unknown action {other}").into()),
    };
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

async fn spawn(socket: &Path) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let launch = LaunchIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        ..LaunchIdentity::default()
    };
    let terminal = SpawnTarget::Terminal(TerminalSpawn {
        cols: 80,
        rows: 24,
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        ..TerminalSpawn::default()
    });
    println!("terminal {}", client.spawn(launch, terminal).await?);
    Ok(())
}

/// The fake agent is built beside this example, one directory up.
fn fake_agent() -> Result<std::path::PathBuf, Failure> {
    let examples = std::env::current_exe()?;
    let debug = examples
        .parent()
        .and_then(Path::parent)
        .ok_or("the example is not inside a target directory")?;
    Ok(debug.join("sikemux-fake-acp-agent"))
}

async fn publish(socket: &Path) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let accounts = serde_json::json!({
        "profiles": [{
            "id": "fake",
            "name": "Fake",
            "provider": "opencode",
            "accent": "#a277ff",
            "executablePath": fake_agent()?,
        }],
    });
    client.call("setAgentAccounts", accounts).await?;
    let project = serde_json::json!({ "id": "tmp", "name": "tmp", "path": std::env::temp_dir() });
    client.call("addProject", project).await?;
    client
        .call(
            "setAgentDefaults",
            serde_json::json!({ "permissionMode": "workspace-write" }),
        )
        .await?;
    println!("published the fake agent");
    Ok(())
}

async fn chat(socket: &Path, prompt: &str) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let (agent, _start) = client
        .start_chat("opencode:fake".into(), "tmp".into(), None)
        .await?;
    println!("chat {agent}");
    if !prompt.is_empty() {
        client
            .ask(ask::AcpPrompt {
                agent_id: agent,
                message_id: None,
                text: prompt.into(),
                paths: Vec::new(),
                context: Vec::new(),
            })
            .await?;
    }
    Ok(())
}

async fn chats(socket: &Path) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&client.ask(ask::AcpList).await?)?
    );
    Ok(())
}

/// Keeps one chat asleep, which the core wakes when a device opens it.
async fn sleepy(socket: &Path) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let agent_id = "agent-sleepy".to_owned();
    let agent = serde_json::json!({
        "id": agent_id,
        "provider": "opencode",
        "project": "tmp",
        "cwd": std::env::temp_dir(),
        "title": "Tidy the docs",
        "permissionMode": "workspace-write",
        "profileId": "fake",
        "asleep": true,
    });
    client
        .call("createAgent", serde_json::json!({ "agent": agent }))
        .await?;
    println!("{agent_id} is asleep");
    Ok(())
}

async fn say(socket: &Path, agent: &str, text: &str) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    client
        .ask(ask::AcpPrompt {
            agent_id: agent.into(),
            message_id: None,
            text: text.into(),
            paths: Vec::new(),
            context: Vec::new(),
        })
        .await?;
    Ok(())
}

async fn endpoint(key_file: &Path) -> Result<Endpoint, Failure> {
    let key = match std::fs::read(key_file) {
        Ok(bytes) => {
            SecretKey::from_bytes(&bytes.try_into().map_err(|_| "the key file is damaged")?)
        }
        Err(_) => {
            let key = SecretKey::generate();
            std::fs::write(key_file, key.to_bytes())?;
            key
        }
    };
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(network::relay_map(
            &network::default_relays(),
        )))
        .secret_key(key)
        .bind()
        .await?;
    tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await?;
    Ok(endpoint)
}

fn core_addr(core: &str) -> Result<EndpointAddr, Failure> {
    Ok(EndpointAddr::new(core.parse()?).with_relay_url(network::DEFAULT_RELAY.parse()?))
}

async fn sessions(key_file: &Path, core: &str) -> Result<(), Failure> {
    let endpoint = endpoint(key_file).await?;
    let started = std::time::Instant::now();
    let (client, _events) = remote::connect(&endpoint, core_addr(core)?).await?;
    println!("connected in {:?}", started.elapsed());
    for session in client.ask(ask::List).await? {
        println!(
            "{} {:?} running={}",
            session.id, session.kind, session.running
        );
    }
    for chat in client.ask(ask::AcpList).await? {
        println!(
            "chat {} {} in {}",
            chat.agent_id,
            chat.provider,
            chat.cwd.display()
        );
    }
    drop(client);
    endpoint.close().await;
    Ok(())
}
