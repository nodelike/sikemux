//! Loads a throwaway core with many chats, tabs and terminals and prints how it holds up,
//! one JSON line per measurement. Never point it at the core you work in.
//!   stress <socket> setup <fake agent> <project folder>
//!   stress <socket> chats <count>                 starts chats with the fake agent, each with its tab
//!   stress <socket> terminals <count>             opens terminals, each with its tab
//!   stress <socket> flood <count> <seconds>       terminals printing as fast as they can
//!   stress <socket> stream <count> <pieces>       that many chats streaming a reply at once
//!   stress <socket> tools <count>                 that many chats waiting on plugin changes, as every
//!                                                 agent's tools server does, then ten quick tool calls
//!   stress <socket> probe                         how long the core takes to answer, now

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use sikemux_client::client::{ClientEvent, CoreClient};
use sikemux_wire::protocol::{
    ask, AgentState, Event, LaunchIdentity, SpawnTarget, TerminalSpawn,
};
use tokio::sync::mpsc::UnboundedReceiver;

type Failure = Box<dyn std::error::Error>;

const PROJECT: &str = "stress";

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let (socket, rest) = words.split_first().ok_or("usage: stress <socket> …")?;
    let (client, events) = CoreClient::connect(Path::new(socket)).await?;
    let client = Arc::new(client);
    match rest {
        ["setup", agent, folder] => setup(&client, agent, folder).await,
        ["chats", count] => chats(&client, count.parse()?).await,
        ["terminals", count] => terminals(&client, count.parse()?).await,
        ["flood", count, seconds] => flood(&client, count.parse()?, seconds.parse()?).await,
        ["stream", count, pieces] => stream(&client, events, count.parse()?, pieces.parse()?).await,
        ["tools", holders] => tools(&client, events, holders.parse()?).await,
        ["probe"] => {
            print_probe(&client, "probe").await;
            Ok(())
        }
        _ => Err("unknown command".into()),
    }
}

async fn setup(client: &CoreClient, agent: &str, folder: &str) -> Result<(), Failure> {
    let accounts = json!({
        "profiles": [{
            "id": "fake", "name": "Fake", "provider": "opencode",
            "accent": "#a277ff", "executablePath": agent,
        }],
    });
    client.call("setAgentAccounts", accounts).await?;
    client
        .call("addProject", json!({ "id": PROJECT, "name": PROJECT, "path": folder }))
        .await?;
    client
        .call("setAgentDefaults", json!({ "permissionMode": "workspace-write" }))
        .await?;
    println!("{}", json!({ "setup": "ok", "core": client.core_pid() }));
    Ok(())
}

fn millis(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.
}

fn spread(mut samples: Vec<f64>) -> serde_json::Value {
    if samples.is_empty() {
        return json!(null);
    }
    samples.sort_by(f64::total_cmp);
    let at = |share: f64| samples[((samples.len() - 1) as f64 * share).round() as usize];
    json!({
        "n": samples.len(),
        "mean": samples.iter().sum::<f64>() / samples.len() as f64,
        "p50": at(0.5), "p95": at(0.95), "max": at(1.),
    })
}

/// The core's resident memory in MB and its CPU share, as `ps` reads them.
fn usage(pid: u32) -> serde_json::Value {
    let out = sikemux_process::user_environment::command("/bin/ps")
        .args(["-o", "rss=,%cpu=", "-p", &pid.to_string()])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let mut fields = out.split_whitespace();
    let rss: f64 = fields.next().and_then(|kb| kb.parse().ok()).unwrap_or(0.);
    let cpu: f64 = fields.next().and_then(|cpu| cpu.parse().ok()).unwrap_or(0.);
    json!({ "rssMb": (rss / 1024.).round(), "cpu": cpu })
}

async fn probe(client: &CoreClient) -> serde_json::Value {
    let started = Instant::now();
    let listed = client.ask(ask::List).await.map(|list| list.len()).unwrap_or(0);
    let list = millis(started);
    let started = Instant::now();
    let chats = client.ask(ask::AcpList).await.map(|list| list.len()).unwrap_or(0);
    let chat_list = millis(started);
    json!({ "listMs": list, "chatListMs": chat_list, "sessions": listed, "chats": chats })
}

async fn print_probe(client: &CoreClient, label: &str) {
    let probe = probe(client).await;
    println!("{}", json!({ "at": label, "probe": probe, "core": usage(client.core_pid()) }));
}

async fn open_tab(client: &CoreClient, kind: &str, target: &str) -> Result<f64, Failure> {
    let started = Instant::now();
    client
        .call(
            "openTab",
            json!({ "project": PROJECT, "kind": kind, "target": target, "behind": true }),
        )
        .await?;
    Ok(millis(started))
}

async fn chats(client: &CoreClient, count: usize) -> Result<(), Failure> {
    let (mut starts, mut tabs, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    for index in 0..count {
        let started = Instant::now();
        match client
            .start_chat("opencode:fake".into(), PROJECT.into(), None)
            .await
        {
            Ok((agent, _)) => {
                starts.push(millis(started));
                match open_tab(client, "agent", &agent).await {
                    Ok(ms) => tabs.push(ms),
                    Err(error) => refused.push(format!("tab {index}: {error}")),
                }
            }
            Err(error) => refused.push(format!("chat {index}: {error}")),
        }
    }
    println!(
        "{}",
        json!({
            "chats": count, "startMs": spread(starts), "tabMs": spread(tabs),
            "refused": refused, "probe": probe(client).await, "core": usage(client.core_pid()),
        })
    );
    Ok(())
}

async fn spawn_terminal(client: &CoreClient) -> Result<String, Failure> {
    let terminal = SpawnTarget::Terminal(TerminalSpawn {
        cols: 120,
        rows: 40,
        cwd: Some(std::env::temp_dir().to_string_lossy().into_owned()),
        ..TerminalSpawn::default()
    });
    let launch = LaunchIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        ..LaunchIdentity::default()
    };
    Ok(client.spawn(launch, terminal).await?.to_string())
}

async fn terminals(client: &CoreClient, count: usize) -> Result<(), Failure> {
    let (mut spawns, mut tabs, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    for index in 0..count {
        let started = Instant::now();
        match spawn_terminal(client).await {
            Ok(id) => {
                spawns.push(millis(started));
                match open_tab(client, "terminal", &id).await {
                    Ok(ms) => tabs.push(ms),
                    Err(error) => refused.push(format!("tab {index}: {error}")),
                }
            }
            Err(error) => refused.push(format!("terminal {index}: {error}")),
        }
    }
    println!(
        "{}",
        json!({
            "terminals": count, "spawnMs": spread(spawns), "tabMs": spread(tabs),
            "refused": refused, "probe": probe(client).await, "core": usage(client.core_pid()),
        })
    );
    Ok(())
}

async fn flood(client: &CoreClient, count: usize, seconds: u64) -> Result<(), Failure> {
    let script = format!(
        "end=$(( $(date +%s) + {seconds} )); while [ $(date +%s) -lt $end ]; do cat /usr/share/dict/words; done"
    );
    let mut refused = Vec::new();
    for index in 0..count {
        match spawn_terminal(client).await {
            Ok(id) => client.write(id.parse()?, format!("{script}\r").as_bytes()).await?,
            Err(error) => refused.push(format!("terminal {index}: {error}")),
        }
    }
    let (mut list, mut cpu, mut rss) = (Vec::new(), Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let probe = probe(client).await;
        list.push(probe["listMs"].as_f64().unwrap_or(0.));
        let core = usage(client.core_pid());
        cpu.push(core["cpu"].as_f64().unwrap_or(0.));
        rss.push(core["rssMb"].as_f64().unwrap_or(0.));
    }
    println!(
        "{}",
        json!({
            "flood": count, "seconds": seconds, "listMs": spread(list), "cpu": spread(cpu),
            "rssMb": spread(rss), "refused": refused,
        })
    );
    Ok(())
}

async fn prompt(client: &CoreClient, agent: &str, text: String) -> Result<(), Failure> {
    let prompt = ask::AcpPrompt {
        agent_id: agent.to_owned(),
        message_id: None,
        text,
        paths: Vec::new(),
        context: Vec::new(),
    };
    client.ask(prompt).await?;
    Ok(())
}

/// The text of the chat changes in `changes` that hold `needle`.
fn said(changes: &serde_json::Value, needle: &str) -> Option<String> {
    let text = changes.to_string();
    let at = text.find(needle)?;
    Some(text[at..].chars().take(120).collect())
}

async fn tools(
    client: &CoreClient,
    mut events: UnboundedReceiver<ClientEvent>,
    holders: usize,
) -> Result<(), Failure> {
    let mut refused = Vec::new();
    for index in 0..holders {
        match client
            .start_chat("opencode:fake".into(), PROJECT.into(), None)
            .await
        {
            Ok((agent, _)) => {
                prompt(client, &agent, r#"tool plugins.changed {"seen":0}"#.into()).await?
            }
            Err(error) => refused.push(format!("chat {index}: {error}")),
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    while events.try_recv().is_ok() {}
    let mut probes = Vec::new();
    for _ in 0..10 {
        let (agent, _) = client
            .start_chat("opencode:fake".into(), PROJECT.into(), None)
            .await?;
        probes.push(agent);
    }
    let started = Instant::now();
    for agent in &probes {
        prompt(client, agent, "tool memory.list {}".into()).await?;
    }
    let mut answers: Vec<(String, f64, String)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while answers.len() < probes.len() && Instant::now() < deadline {
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(200), events.recv()).await
        else {
            continue;
        };
        if let ClientEvent::Event(Event::Chat { agent_id, changes, .. }) = event {
            if !probes.contains(&agent_id) || answers.iter().any(|(id, ..)| *id == agent_id) {
                continue;
            }
            if let Some(text) = said(&changes, "tool answered").or_else(|| said(&changes, "tool failed")) {
                answers.push((agent_id, millis(started), text));
            }
        }
    }
    let failed = answers.iter().filter(|(_, _, text)| text.starts_with("tool failed")).count();
    println!(
        "{}",
        json!({
            "holders": holders, "probes": probes.len(), "answered": answers.len() - failed,
            "failed": failed, "silent": probes.len() - answers.len(),
            "answerMs": spread(answers.iter().map(|(_, ms, _)| *ms).collect()),
            "firstFailure": answers.iter().find(|(_, _, text)| text.starts_with("tool failed")).map(|(_, _, text)| text.clone()),
            "refused": refused, "core": usage(client.core_pid()),
        })
    );
    Ok(())
}

async fn stream(
    client: &CoreClient,
    mut events: UnboundedReceiver<ClientEvent>,
    count: usize,
    pieces: usize,
) -> Result<(), Failure> {
    let mut agents = Vec::new();
    let mut refused = Vec::new();
    for index in 0..count {
        match client
            .start_chat("opencode:fake".into(), PROJECT.into(), None)
            .await
        {
            Ok((agent, _)) => agents.push(agent),
            Err(error) => refused.push(format!("chat {index}: {error}")),
        }
    }
    while events.try_recv().is_ok() {}
    let started = Instant::now();
    for agent in &agents {
        let prompt = ask::AcpPrompt {
            agent_id: agent.clone(),
            message_id: None,
            text: format!("stream {pieces}"),
            paths: Vec::new(),
            context: Vec::new(),
        };
        if let Err(error) = client.ask(prompt).await {
            refused.push(format!("prompt {agent}: {error}"));
        }
    }
    let sent = millis(started);
    let mut waiting: HashSet<String> = agents.iter().cloned().collect();
    let mut worked = HashSet::new();
    let (mut list, mut cpu, mut changes) = (Vec::new(), Vec::new(), 0usize);
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut next_probe = Instant::now();
    while !waiting.is_empty() && Instant::now() < deadline {
        if Instant::now() >= next_probe {
            let probe = probe(client).await;
            list.push(probe["listMs"].as_f64().unwrap_or(0.));
            cpu.push(usage(client.core_pid())["cpu"].as_f64().unwrap_or(0.));
            next_probe = Instant::now() + Duration::from_millis(500);
        }
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(100), events.recv()).await
        else {
            continue;
        };
        match event {
            ClientEvent::Event(Event::Chat { .. }) => changes += 1,
            ClientEvent::Event(Event::AgentState(state)) => match state.state {
                AgentState::Working => {
                    worked.insert(state.agent_id);
                }
                AgentState::Idle if worked.contains(&state.agent_id) => {
                    waiting.remove(&state.agent_id);
                }
                _ => {}
            },
            _ => {}
        }
    }
    println!(
        "{}",
        json!({
            "stream": count, "pieces": pieces, "promptsSentMs": sent, "allDoneMs": millis(started),
            "unfinished": waiting.len(), "chatEvents": changes, "listMs": spread(list),
            "cpu": spread(cpu), "refused": refused, "core": usage(client.core_pid()),
        })
    );
    Ok(())
}
