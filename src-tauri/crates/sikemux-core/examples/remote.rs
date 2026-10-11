//! Talks to a core from another machine, the way the phone app does, over the
//! real network.
//!
//! As the Mac, against a core's socket:
//!   remote mac <socket> on | off | allow | status
//!   remote mac <socket> spawn
//!   remote mac <socket> publish            offers the fake agent to devices
//!   remote mac <socket> chat [prompt]      starts a chat with it
//!   remote mac <socket> recent <title>…    lists saved chats of the fake agent as recent
//!   remote mac <socket> say <agent> <text>
//!   remote mac <socket> backdrop on|off [image.jpg]  publishes the pane backdrop
//!   remote mac <socket> palette <name=colour>…  publishes theme colours
//!   remote mac <socket> chats              lists the chats running on the core
//!   remote mac <socket> sleepy             lists a sleeping chat and wakes it when asked
//! As a device, keeping its key in `<key-file>`:
//!   remote device <key-file> sessions <core-id>

use std::path::Path;
use std::time::Duration;

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use sikemux_core::accounts::network;
use sikemux_core::client::{ClientEvent, CoreClient};
use sikemux_core::protocol::{
    BackdropImage, ChatLaunch, ChatLauncher, DeviceAccess, Event, LaunchIdentity, ProjectInfo,
    PublishedChat, PublishedRecent, SpawnTarget, TerminalSpawn,
};
use sikemux_core::remote::{self, SecretKey};

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
        ["mac", socket, "recent", titles @ ..] => recent(Path::new(socket), titles).await,
        ["mac", socket, "sleepy"] => sleepy(Path::new(socket)).await,
        ["mac", socket, "chats"] => chats(Path::new(socket)).await,
        ["mac", socket, "palette", colours @ ..] => palette(Path::new(socket), colours).await,
        ["mac", socket, "backdrop", texture, image @ ..] => backdrop(Path::new(socket), *texture == "on", image.first().copied()).await,
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
    let launcher = ChatLauncher {
        id: "opencode".into(),
        provider: "opencode".into(),
        label: "OpenCode".into(),
        program: fake_agent()?,
        args: vec!["acp".into()],
        env: Default::default(),
        permission_mode: "workspace-write".into(),
        account: None,
        fallbacks: Vec::new(),
        status: None,
    };
    let project = ProjectInfo {
        id: "tmp".into(),
        name: "tmp".into(),
        path: std::env::temp_dir(),
    };
    client
        .publish_workspace(vec![project], vec![launcher])
        .await?;
    println!("published the fake agent");
    Ok(())
}

async fn chat(socket: &Path, prompt: &str) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let (agent, _start) = client
        .start_chat("opencode".into(), "tmp".into(), None)
        .await?;
    println!("chat {agent}");
    if !prompt.is_empty() {
        client
            .acp_prompt(agent, None, prompt.into(), Vec::new(), Vec::new())
            .await?;
    }
    Ok(())
}

/// Each title becomes a saved chat an hour older than the one before.
async fn recent(socket: &Path, titles: &[&str]) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let chats = titles
        .iter()
        .enumerate()
        .map(|(index, title)| PublishedRecent {
            launcher: "opencode".into(),
            provider: "opencode".into(),
            session_id: format!("saved-{index}"),
            title: (*title).into(),
            cwd: std::env::temp_dir(),
            active_at: now - index as u64 * 3_600_000,
        })
        .collect();
    client.publish_recent(chats).await?;
    println!("published {} recent chats", titles.len());
    Ok(())
}

async fn backdrop(socket: &Path, texture: bool, image: Option<&str>) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let picture = match image {
        Some(path) => {
            let bytes = std::fs::read(path)?;
            Some(BackdropImage {
                id: format!("{:x}", bytes.len()),
                data_url: format!("data:image/jpeg;base64,{}", base64(&bytes)),
            })
        }
        None => None,
    };
    client.publish_backdrop(texture, picture).await?;
    println!("published the backdrop");
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

async fn palette(socket: &Path, colours: &[&str]) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    let palette = colours
        .iter()
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, colour)| (name.to_owned(), colour.to_owned()))
        .collect();
    client.publish_palette(palette).await?;
    println!("published the palette");
    Ok(())
}

async fn chats(socket: &Path) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&client.acp_list().await?)?
    );
    Ok(())
}

/// Stands in for the app: keeps one chat asleep until a device opens it.
async fn sleepy(socket: &Path) -> Result<(), Failure> {
    let (client, mut events) = CoreClient::connect(socket).await?;
    let agent_id = "agent-sleepy".to_owned();
    client
        .publish_agents(
            vec![PublishedChat {
                agent_id: agent_id.clone(),
                provider: "opencode".into(),
                title: Some("Tidy the docs".into()),
                cwd: std::env::temp_dir(),
                asleep: true,
            }],
            Default::default(),
        )
        .await?;
    println!("{agent_id} is asleep");
    while let Some(event) = events.recv().await {
        let ClientEvent::Event(Event::WakeChat { agent_id: woken }) = event else {
            continue;
        };
        let launch = ChatLaunch {
            agent_id: woken.clone(),
            provider: "opencode".into(),
            cwd: std::env::temp_dir(),
            program: fake_agent()?,
            args: vec!["acp".into()],
            env: Default::default(),
            mcp_servers: Vec::new(),
            resume_id: None,
            permission_mode: "workspace-write".into(),
            model: None,
            effort: None,
            account: None,
            fallbacks: Vec::new(),
        };
        client.acp_start(launch).await?;
        println!("woke {woken}");
    }
    Ok(())
}

async fn say(socket: &Path, agent: &str, text: &str) -> Result<(), Failure> {
    let (client, _events) = CoreClient::connect(socket).await?;
    client
        .acp_prompt(agent.into(), None, text.into(), Vec::new(), Vec::new())
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
    for session in client.list().await? {
        println!(
            "{} {:?} running={}",
            session.id, session.kind, session.running
        );
    }
    for chat in client.acp_list().await? {
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
