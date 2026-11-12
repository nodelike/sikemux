//! `sikemux chat`: list, start, message, read and stop chats from a terminal. In an agent's
//! terminal it acts as that agent; anywhere else it asks the person to let it in once.

use std::io::Read as _;

use serde_json::{json, Value};
use sikemux_core::cli::protocol::{
    OUTSIDE_CALL_METHOD, OUTSIDE_SIGN_IN_METHOD, OUTSIDE_SIGN_IN_WAIT_METHOD,
};

use super::*;

const TOKEN_FILE: (&str, &str) = ("chat-token", "json");
/// The core answers a sign-in wait well within this, until the person answers.
const SIGN_IN_ROUNDS: usize = 4;
const SAY_WAITED: u64 = 50_000;

struct Options {
    words: Vec<String>,
    json: bool,
    wait: bool,
    all: bool,
    agent: Option<String>,
    model: Option<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut parsed = Options {
        words: Vec::new(),
        json: false,
        wait: false,
        all: false,
        agent: None,
        model: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--wait" | "-w" => parsed.wait = true,
            "--all" => parsed.all = true,
            "--agent" => parsed.agent = Some(args.next().ok_or("--agent needs a name")?.clone()),
            "--model" => parsed.model = Some(args.next().ok_or("--model needs a name")?.clone()),
            flag if flag.starts_with("--") => return Err(format!("chat takes no {flag}")),
            word => parsed.words.push(word.to_owned()),
        }
    }
    Ok(parsed)
}

/// `-` reads the message from stdin.
fn message(word: &str) -> Result<String, String> {
    if word != "-" {
        return Ok(word.to_owned());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| format!("could not read the message from stdin: {error}"))?;
    Ok(text)
}

pub(super) fn chat(args: &[String]) -> Result<i32, String> {
    let Some((command, rest)) = args.split_first() else {
        print!("{}", command_help("chat").unwrap_or_default());
        return Ok(0);
    };
    let options = options(rest)?;
    let caller = Caller::find()?;
    let words = &options.words;
    let word = |index: usize, what: &str| {
        words
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("chat {command} needs {what}; see `sikemux help chat`"))
    };
    let answer = match command.as_str() {
        "list" => caller.list(options.all)?,
        "start" => {
            let project = project_named(word(0, "a project")?)?;
            let text = message(word(1, "a message")?)?;
            let started = caller.start(&project, &text, &options)?;
            if !options.wait {
                return print_answer(&started, options.json, |started| {
                    println!("{}", label(started));
                });
            }
            let chat = started["agentId"].as_str().unwrap_or_default().to_owned();
            caller.wait_and_read(&chat)?
        }
        "send" => {
            let chat = word(0, "a chat")?;
            let text = message(word(1, "a message")?)?;
            let sent = caller.send(chat, &text)?;
            if !options.wait {
                return print_answer(&sent, options.json, |_| {});
            }
            caller.wait_and_read(chat)?
        }
        "read" => caller.read(word(0, "a chat")?)?,
        "stop" => caller.stop(word(0, "a chat")?)?,
        other => {
            return Err(format!(
                "chat has no command {other}; see `sikemux help chat`"
            ))
        }
    };
    let ended = answer["status"].as_str().map(str::to_owned);
    print_answer(&answer, options.json, |answer| {
        if let Some(chats) = answer["chats"].as_array().or(answer["agents"].as_array()) {
            print_chats(chats);
        } else if answer.get("turns").is_some() {
            print_turns(answer);
        }
    })?;
    Ok(i32::from(ended.as_deref() == Some("stopped")))
}

fn print_answer(answer: &Value, json: bool, plain: impl Fn(&Value)) -> Result<i32, String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(answer).unwrap_or_default()
        );
    } else {
        plain(answer);
    }
    Ok(0)
}

fn label(chat: &Value) -> String {
    let name = chat["chat"].as_str().or(chat["name"].as_str());
    match name {
        Some(name) => format!("@{}", name.trim_start_matches('@')),
        None => chat["agentId"].as_str().unwrap_or_default().to_owned(),
    }
}

fn print_chats(chats: &[Value]) {
    for chat in chats {
        let mut line = format!(
            "{:<16} {:<10}",
            label(chat),
            chat["status"].as_str().unwrap_or_default()
        );
        if let Some(project) = chat["project"].as_str() {
            line.push_str(&format!(" {project:<16}"));
        }
        if let Some(title) = chat["title"].as_str() {
            line.push_str(&format!(" {title}"));
        }
        println!("{}", line.trim_end());
    }
}

fn print_turns(answer: &Value) {
    for turn in answer["turns"].as_array().into_iter().flatten() {
        for asked in turn["asked"].as_array().into_iter().flatten() {
            println!(
                "> {}: {}",
                asked["from"].as_str().unwrap_or_default(),
                asked["text"].as_str().unwrap_or_default()
            );
        }
        for said in turn["said"].as_array().into_iter().flatten() {
            println!("{}", said.as_str().unwrap_or_default());
        }
        if let Some(tools) = turn["tools"].as_str() {
            println!("({tools})");
        }
        if let Some(answer) = turn["answer"].as_str() {
            println!("{answer}");
        }
        println!();
    }
    if let Some(note) = answer["note"].as_str() {
        eprintln!("{note}");
    }
}

/// `.` is the project that holds this folder; anything else is a project's name, id or folder.
fn project_named(word: &str) -> Result<String, String> {
    match word {
        "." => current_project(),
        word => Ok(word.to_owned()),
    }
}

/// Who `sikemux chat` acts as.
enum Caller {
    /// The agent whose terminal this is.
    Agent(String),
    /// A program outside every agent, with the token the person let it in with.
    Outside(String),
}

impl Caller {
    fn find() -> Result<Caller, String> {
        if let Ok(agent) = env::var(vars::AGENT_ID) {
            if !agent.is_empty() {
                return Ok(Caller::Agent(agent));
            }
        }
        if let Some(token) = saved_token() {
            return Ok(Caller::Outside(token));
        }
        Ok(Caller::Outside(sign_in()?))
    }

    fn tool(&self, outside: &str, agent: &str, arguments: Value) -> Result<Value, String> {
        match self {
            Caller::Agent(id) => call(agent, Some(id), &arguments),
            Caller::Outside(token) => {
                let params = json!({ "token": token, "tool": outside, "arguments": arguments });
                call(OUTSIDE_CALL_METHOD, None, &params).inspect_err(|error| {
                    if error.contains("sign in again") {
                        forget_token();
                    }
                })
            }
        }
    }

    fn list(&self, all: bool) -> Result<Value, String> {
        match self {
            Caller::Agent(_) => self.tool("", "agent.list", json!({ "allProjects": all })),
            Caller::Outside(_) => {
                let everything = || self.tool("chat_list", "", json!({}));
                if all {
                    return everything();
                }
                let here = self.tool("chat_list", "", json!({ "project": current_project()? }));
                here.or_else(|error| match error.contains("No project") {
                    true => everything(),
                    false => Err(error),
                })
            }
        }
    }

    fn start(&self, project: &str, text: &str, options: &Options) -> Result<Value, String> {
        match self {
            Caller::Agent(_) => self.tool(
                "",
                "agent.spawn",
                json!({ "prompt": text, "cli": options.agent, "model": options.model }),
            ),
            Caller::Outside(_) => self.tool(
                "chat_start",
                "",
                json!({ "project": project, "prompt": text, "agent": options.agent, "model": options.model }),
            ),
        }
    }

    fn send(&self, chat: &str, text: &str) -> Result<Value, String> {
        self.tool(
            "chat_send",
            "agent.send",
            match self {
                Caller::Agent(_) => json!({ "to": chat, "text": text }),
                Caller::Outside(_) => json!({ "chat": chat, "text": text }),
            },
        )
    }

    fn read(&self, chat: &str) -> Result<Value, String> {
        self.tool(
            "chat_read",
            "agent.read",
            match self {
                Caller::Agent(_) => json!({ "agent": chat }),
                Caller::Outside(_) => json!({ "chat": chat }),
            },
        )
    }

    fn stop(&self, chat: &str) -> Result<Value, String> {
        match self {
            Caller::Agent(_) => Err(
                "an agent cannot stop another's turn; ask the person, or send it a message".into(),
            ),
            Caller::Outside(_) => self.tool("chat_stop", "", json!({ "chat": chat })),
        }
    }

    /// Waits for the chat's turn to end, then reads it.
    fn wait_and_read(&self, chat: &str) -> Result<Value, String> {
        if matches!(self, Caller::Agent(_)) {
            return Err("in an agent's terminal, wait with agent_wait instead of --wait".into());
        }
        loop {
            let waited = self.tool(
                "chat_wait",
                "",
                json!({ "chat": chat, "timeoutMs": SAY_WAITED }),
            )?;
            if waited["timedOut"].as_bool() != Some(true) {
                break;
            }
        }
        self.read(chat)
    }
}

fn call(method: &str, agent_id: Option<&str>, params: &Value) -> Result<Value, String> {
    attempt(
        &endpoint_to_read()?,
        &current_project()?,
        agent_id,
        method,
        params,
        TOOLS_REPLY_TIMEOUT,
    )
    .map_err(|(Failure::NotSent(message) | Failure::Sent(message))| message)
}

fn token_path() -> Result<PathBuf, String> {
    sikemux_process::paths::config_file(TOKEN_FILE.0, TOKEN_FILE.1)
        .ok_or_else(|| "home directory is unavailable".into())
}

fn saved_token() -> Option<String> {
    let saved: Value = serde_json::from_slice(&fs::read(token_path().ok()?).ok()?).ok()?;
    saved["token"].as_str().map(str::to_owned)
}

fn forget_token() {
    if let Ok(path) = token_path() {
        let _ = fs::remove_file(path);
    }
}

/// Asks the person to let this terminal in, and keeps the token they allow.
fn sign_in() -> Result<String, String> {
    let name = format!(
        "Terminal ({})",
        env::var("TERM_PROGRAM").unwrap_or_else(|_| "sikemux chat".into())
    );
    let asked = call(OUTSIDE_SIGN_IN_METHOD, None, &json!({ "name": name }))?;
    let id = asked["id"]
        .as_str()
        .ok_or("Sikemux did not start the sign-in")?;
    eprintln!(
        "Allow this terminal in Sikemux: check that it shows {}, then choose what it may do.",
        asked["check"].as_str().unwrap_or_default()
    );
    for _ in 0..SIGN_IN_ROUNDS {
        let answer = call(OUTSIDE_SIGN_IN_WAIT_METHOD, None, &json!({ "id": id }))?;
        let Some(token) = answer["token"].as_str() else {
            continue;
        };
        let path = token_path()?;
        sikemux_process::files::write_atomic(
            &path,
            json!({ "token": token }).to_string().as_bytes(),
            true,
        )
        .map_err(|error| format!("could not keep the sign-in in {}: {error}", path.display()))?;
        eprintln!("Signed in. Remove it any time in Sikemux's settings.");
        return Ok(token.to_owned());
    }
    Err("Nobody answered in Sikemux; run the command again".into())
}
