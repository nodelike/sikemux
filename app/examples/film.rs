//! Films the app for posts, the way the launch reel was made: a scene moves a drawn cursor and a
//! camera over the real window, drawn offscreen a sixtieth of a second at a time, while a
//! throwaway core and a stand-in Claude answer it. `film/film.sh` builds what it needs and runs
//! it; scenes live in `film/scenes`.
//!   cargo run --example film -- <scene.film> --core <sikemux-host> --claude <sikemux-fake-claude>
//!     --work <folder> --out <clip.mp4, or a folder for stills> [--events <file.json>]
//!     [--wallpaper <png>] [--lockup <png>] [--height 1440] [--blur] [--draft]
//!     [--stills 1.5,4] [--grid]
//!
//! A scene is a step a line, `#` starting a comment; a step ending in `&` plays on while the
//! next ones start. A place is in window points: `x,y`, a box `x,y,w,h`, or the quoted label of
//! a control the window reports. Times are in seconds.
//!   stage W H / window W H      the frame and the window in its middle, in points
//!   replies FILE                the stand-in Claude's scripted replies (see sikemux-fake-claude)
//!   project NAME FOLDER         copies FOLDER, beside the scene, into a new git project
//!   chat NAME                   opens a Claude chat named NAME in the last project
//!   open FILE                   opens FILE of the last project in a tab
//!   settle S                    lets the core and the window catch up, unfilmed
//!   hold S                      gives the core S real seconds with the film's time stopped, so
//!                               an answer that takes a round trip lands between two frames
//!   record                      starts filming; the steps before it play unseen
//!   cursor P / move P [S]       puts the cursor at P, or glides it there
//!   click [P] / dclick [P] / rclick [P]    clicks, gliding to P first
//!   press / release / drag P [S]           holds the button, lets it go, or drags to P
//!   type "TEXT" [PER_SECOND]    types at a person's pace
//!   key NAME                    presses a key such as enter or cmd-k
//!   zoom P SCALE [S] / zoom BOX [S] / zoom out [S]    moves the camera
//!   pinch FACTOR [S]            pinches at the cursor, above 1 to zoom in
//!   pan DX,DY [S] / scroll DY [S]          two-finger scrolls at the cursor
//!   shape arrow|hand|beam|auto  which pointer is drawn
//!   wait S / mark KIND / still NAME / end [HOLD]

use std::cell::RefCell;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _};
use film_frame::{Compositor, Picture, Pointer, Setup, Shape, View};
use gpui::{
    point, px, size, AnyWindowHandle, AppContext, HeadlessAppContext, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, PlatformInput, Role,
    ScrollDelta, ScrollEvent, ScrollSource, TouchPhase,
};
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_gpui::{core, icons, theme, workbench};

const FPS: f64 = 60.;

#[derive(Clone, Debug)]
struct Word {
    text: String,
    quoted: bool,
}

#[derive(Debug)]
struct Step {
    line: usize,
    verb: String,
    words: Vec<Word>,
    background: bool,
}

fn read_scene(text: &str) -> anyhow::Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let mut words = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() {
                chars.next();
            } else if next == '#' {
                break;
            } else if next == '"' {
                chars.next();
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => text.extend(chars.next()),
                        _ => text.push(c),
                    }
                }
                words.push(Word { text, quoted: true });
            } else {
                let mut text = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() {
                        break;
                    }
                    text.push(c);
                    chars.next();
                }
                words.push(Word {
                    text,
                    quoted: false,
                });
            }
        }
        let background = words
            .last()
            .is_some_and(|word| !word.quoted && word.text == "&");
        if background {
            words.pop();
        }
        if words.is_empty() {
            continue;
        }
        let verb = words.remove(0).text;
        steps.push(Step {
            line: at + 1,
            verb,
            words,
            background,
        });
    }
    Ok(steps)
}

impl Step {
    fn number(&self, at: usize) -> Option<f32> {
        self.words.get(at).and_then(|word| word.text.parse().ok())
    }

    fn numbers(&self, at: usize) -> Option<Vec<f32>> {
        self.words.get(at).and_then(|word| {
            word.text
                .split(',')
                .map(|part| part.trim().parse().ok())
                .collect()
        })
    }

    fn text(&self, at: usize) -> anyhow::Result<&str> {
        self.words
            .get(at)
            .map(|word| word.text.as_str())
            .ok_or_else(|| anyhow!("line {}: {} needs more", self.line, self.verb))
    }
}

struct Args {
    scene: PathBuf,
    core: PathBuf,
    claude: PathBuf,
    work: PathBuf,
    out: PathBuf,
    events: Option<PathBuf>,
    wallpaper: Option<PathBuf>,
    lockup: Option<PathBuf>,
    height: u32,
    blur: bool,
    draft: bool,
    stills: Option<Vec<f64>>,
    grid: bool,
    world: Value,
}

fn args() -> anyhow::Result<Args> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        all.iter()
            .position(|arg| arg == name)
            .and_then(|at| all.get(at + 1).cloned())
    };
    let flag = |name: &str| all.iter().any(|arg| arg == name);
    let path = |name: &str| {
        option(name)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("{name} is needed"))
    };
    let draft = flag("--draft");
    Ok(Args {
        scene: all
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("which scene?"))?,
        core: path("--core")?,
        claude: path("--claude")?,
        work: path("--work")?,
        out: path("--out")?,
        events: option("--events").map(PathBuf::from),
        wallpaper: option("--wallpaper").map(PathBuf::from),
        lockup: option("--lockup").map(PathBuf::from),
        height: option("--height")
            .and_then(|height| height.parse().ok())
            .unwrap_or(if draft { 1080 } else { 1440 }),
        blur: flag("--blur"),
        draft,
        stills: option("--stills").map(|list| {
            list.split(',')
                .filter_map(|seconds| seconds.trim().parse().ok())
                .collect()
        }),
        grid: flag("--grid"),
        world: match option("--world") {
            Some(path) => serde_json::from_str(&std::fs::read_to_string(&path)?)
                .with_context(|| format!("reading {path}"))?,
            None => Value::Null,
        },
    })
}

/// The throwaway core the film talks to, started by this process so a dev core trusts it.
struct World {
    core: Child,
    runtime: tokio::runtime::Runtime,
    client: CoreClient,
    _events: Box<dyn std::any::Any>,
    socket: PathBuf,
    work: PathBuf,
    home: PathBuf,
    scenes: PathBuf,
    project: Option<(String, PathBuf)>,
    claude: PathBuf,
    world: Value,
}

impl World {
    fn start(args: &Args, replies: Option<PathBuf>, clock: &Path) -> anyhow::Result<Self> {
        let work = &args.work;
        let home = work.join("home");
        let claude = work.join("claude");
        for folder in [&home, &claude, &work.join("data")] {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(
            claude.join("fake-account.json"),
            json!({ "loggedIn": true, "authMethod": "claude.ai", "email": "edon@acme.dev", "subscriptionType": "max" })
                .to_string(),
        )?;
        std::fs::write(
            claude.join("fake-usage.json"),
            usage(&args.world["account"]["usage"]).to_string(),
        )?;
        let socket = work.join("core.sock");
        let _ = std::fs::remove_file(&socket);
        let log = std::fs::File::create(work.join("core.log"))?;
        let mut command = sikemux_process::user_environment::command(&args.core);
        command
            .arg("core")
            .arg("--socket")
            .arg(&socket)
            .arg("--data-dir")
            .arg(work.join("data"))
            .arg("--cli-endpoint")
            .arg(work.join("cli.json"))
            .env("HOME", &home)
            .env("SIKEMUX_CORE_SOCKET", &socket)
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .env("FAKE_CLAUDE_CLOCK", clock)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0);
        if let Some(replies) = &replies {
            command.env("FAKE_CLAUDE_SCRIPT", replies);
        }
        let core = command.spawn().context("starting the core")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let started = Instant::now();
        let (client, events) = loop {
            match runtime.block_on(CoreClient::connect(&socket)) {
                Ok(connected) => break connected,
                Err(error) if started.elapsed() > Duration::from_secs(30) => {
                    bail!("the core never answered at {}: {error}", socket.display())
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        let profile = json!({
            "id": "fake", "provider": "claude", "accent": "#d97757",
            "name": args.world["account"]["name"].as_str().unwrap_or("Claude"),
            "executablePath": args.claude, "configPath": claude,
        });
        runtime.block_on(client.call("setAgentAccounts", json!({ "profiles": [profile] })))?;
        runtime.block_on(client.call(
            "selectAgentAccount",
            json!({ "provider": "claude", "id": "fake" }),
        ))?;
        let version = client.hello().build.version.clone();
        let mut preferences = vec![
            ("onboardingComplete".to_owned(), json!(true)),
            ("lastSeenVersion".to_owned(), json!(version)),
        ];
        if let Some(kept) = args.world["preferences"].as_object() {
            preferences.extend(
                kept.iter()
                    .map(|(key, value)| (key.clone(), expand_home(value))),
            );
        }
        for (key, value) in preferences {
            runtime
                .block_on(client.call("setPreference", json!({ "key": key, "value": value })))?;
        }
        if args.world["faceStyle"].is_object() {
            runtime.block_on(client.call("setFaceStyle", args.world["faceStyle"].clone()))?;
        }
        Ok(Self {
            core,
            runtime,
            client,
            _events: Box::new(events),
            socket,
            work: work.clone(),
            home,
            scenes: args
                .scene
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            project: None,
            claude,
            world: args.world.clone(),
        })
    }

    /// A year of work in `path` for the activity calendar: busier on weekdays and lately.
    fn seed_activity(&self, path: &Path) -> anyhow::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        let day = 86_400_000i64;
        let mut seed = 11u32;
        let mut random = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f64 / (1u32 << 24) as f64
        };
        let place = path.to_string_lossy().replace('\'', "''");
        let mut sql = String::from(
            "CREATE TABLE IF NOT EXISTS launches (at_ms INTEGER NOT NULL, agent TEXT NOT NULL, \
             project TEXT NOT NULL, transport TEXT NOT NULL, resumed INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS turns (started_ms INTEGER NOT NULL, ended_ms INTEGER NOT NULL, \
             agent TEXT NOT NULL, project TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS commits (sha TEXT PRIMARY KEY, at_ms INTEGER NOT NULL, \
             project TEXT NOT NULL, agent TEXT);
             CREATE TABLE IF NOT EXISTS tokens (at_ms INTEGER NOT NULL, agent TEXT NOT NULL, \
             project TEXT NOT NULL, input INTEGER NOT NULL, output INTEGER NOT NULL, \
             cache_read INTEGER NOT NULL, cache_write INTEGER NOT NULL);
             BEGIN;\n",
        );
        for back in 0..365i64 {
            let weekday = (now / day - back + 4).rem_euclid(7) < 5;
            let lately = 1. - back as f64 / 500.;
            if random() > if weekday { 0.9 } else { 0.4 } * lately {
                continue;
            }
            let at = now - back * day - (random() * 8. * 3_600_000.) as i64;
            let busy = lately * (0.3 + random());
            for turn in 0..(2. + busy * 14.) as i64 {
                let started = at + turn * 1_200_000;
                sql += &format!(
                    "INSERT INTO turns VALUES ({started}, {}, 'claude', '{place}');\n",
                    started + (busy * 900_000.) as i64 + 60_000,
                );
            }
            sql += &format!(
                "INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write) \
                 VALUES ({at}, 'claude', '{place}', {}, {}, 0, {});\n",
                (busy * 900_000.) as i64,
                (busy * 300_000.) as i64,
                (busy * 1_500_000.) as i64,
            );
            for commit in 0..(busy * 6.) as i64 {
                sql += &format!(
                    "INSERT OR IGNORE INTO commits VALUES ('{back:x}{commit:x}', {}, '{place}', 'claude');\n",
                    at + commit * 600_000,
                );
            }
        }
        sql += "COMMIT;\n";
        let mut sqlite = sikemux_process::user_environment::command("sqlite3")
            .arg(self.work.join("data").join("activity.sqlite3"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        sqlite
            .stdin
            .take()
            .ok_or_else(|| anyhow!("sqlite3 took no input"))?
            .write_all(sql.as_bytes())?;
        if !sqlite.wait()?.success() {
            bail!("could not seed the activity calendar");
        }
        Ok(())
    }

    /// Fills the first project's rail and strip the way a working day leaves them: agents
    /// with their titles and teammates, some still working and some done and unread, chats
    /// closed earlier among the recent ones and one of them pinned, and tabs of other kinds.
    fn populate(&self, project: &str, path: &Path) -> anyhow::Result<()> {
        let world = &self.world;
        let path = &path.canonicalize()?;
        let transcripts = claude_project_dir(&self.claude, path);
        std::fs::create_dir_all(&transcripts)?;
        let mut pinned = Vec::new();
        for (at, chat) in world["recent"].as_array().into_iter().flatten().enumerate() {
            let session = session_id(at);
            let title = chat["title"].as_str().unwrap_or("Chat");
            let file = transcripts.join(format!("{session}.jsonl"));
            let lines = [
                json!({ "type": "user", "uuid": session_id(at + 100), "sessionId": session, "cwd": path,
                    "message": { "role": "user", "content": [{ "type": "text", "text": title }] } }),
                json!({ "type": "custom-title", "customTitle": title, "sessionId": session }),
            ];
            std::fs::write(
                &file,
                lines
                    .iter()
                    .map(|line| format!("{line}\n"))
                    .collect::<String>(),
            )?;
            let minutes = 20 + at as u64 * 25;
            sikemux_process::user_environment::command("touch")
                .args(["-A", &format!("-{:02}{:02}00", minutes / 60, minutes % 60)])
                .arg(&file)
                .status()?;
            if chat["pinned"] == true {
                pinned.push(format!("claude:{session}"));
            }
        }
        self.seed_activity(path)?;
        let mut named = std::collections::HashMap::new();
        let agents: Vec<&Value> = world["agents"].as_array().into_iter().flatten().collect();
        for agent in &agents {
            let (chat, _) = self.runtime.block_on(self.client.start_chat(
                "claude:fake".into(),
                project.to_owned(),
                None,
            ))?;
            if let Some(name) = agent["name"].as_str() {
                self.call("renameAgent", json!({ "agentId": chat, "name": name }))?;
                named.insert(name.to_owned(), chat.clone());
            }
            let prompt = match agent["state"].as_str() {
                Some("working") => Some("hold 3600000".to_owned()),
                Some("done") => Some(agent["title"].as_str().unwrap_or("done").to_owned()),
                _ => None,
            };
            if let Some(prompt) = prompt {
                self.runtime.block_on(self.client.acp_prompt(
                    chat.clone(),
                    None,
                    prompt,
                    Vec::new(),
                    Vec::new(),
                ))?;
            }
        }
        std::thread::sleep(Duration::from_secs(2));
        for agent in &agents {
            let Some(chat) = agent["name"].as_str().and_then(|name| named.get(name)) else {
                continue;
            };
            let mut patch = json!({ "title": agent["title"], "renamed": true });
            if let Some(lead) = agent["lead"].as_str().and_then(|lead| named.get(lead)) {
                patch["lead"] = json!(lead);
            }
            self.call("updateAgent", json!({ "agentId": chat, "patch": patch }))?;
        }
        for tab in world["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let opened = match tab {
                "terminal" => self.call("newTerminal", json!({ "project": project })),
                "git" => self.call(
                    "openTab",
                    json!({ "project": project, "kind": "git", "target": path }),
                ),
                file => self.call(
                    "openTab",
                    json!({ "project": project, "kind": "files", "target": path.join(file) }),
                ),
            };
            if let Err(error) = opened {
                eprintln!("film: could not open the {tab} tab: {error}");
            }
        }
        for recent in pinned {
            let started = Instant::now();
            while let Err(error) = self.call("pinChat", json!({ "recent": recent, "pinned": true }))
            {
                if started.elapsed() > Duration::from_secs(10) {
                    eprintln!("film: could not pin {recent}: {error}");
                    break;
                }
                let _ = self.call("recentChats", json!({}));
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        Ok(())
    }

    fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        Ok(self.runtime.block_on(self.client.call(method, params))?)
    }

    fn project(&self) -> anyhow::Result<&(String, PathBuf)> {
        self.project
            .as_ref()
            .ok_or_else(|| anyhow!("add a project first"))
    }

    /// Does `step` when it is the core's to do.
    fn take(&mut self, step: &Step) -> anyhow::Result<bool> {
        match step.verb.as_str() {
            "project" => {
                let name = step.text(0)?.to_owned();
                let from = self.scenes.join(step.text(1)?);
                let path = self.work.join("projects").join(&name);
                std::fs::create_dir_all(path.parent().unwrap_or(&self.work))?;
                let copied = sikemux_process::user_environment::command("cp")
                    .arg("-R")
                    .arg(&from)
                    .arg(&path)
                    .status()?;
                if !copied.success() {
                    bail!("could not copy {}", from.display());
                }
                let git = |words: &[&str]| -> anyhow::Result<()> {
                    let done = sikemux_process::user_environment::command("git")
                        .args(["-c", "user.name=edon", "-c", "user.email=edon@acme.dev"])
                        .args(words)
                        .current_dir(&path)
                        .env("HOME", &self.home)
                        .stdout(Stdio::null())
                        .status()?;
                    if !done.success() {
                        bail!("git {} failed", words.join(" "));
                    }
                    Ok(())
                };
                git(&["init", "-q", "-b", "main"])?;
                git(&["add", "-A"])?;
                git(&["commit", "-q", "-m", "Start the shop"])?;
                self.call(
                    "addProject",
                    json!({ "id": name, "name": name, "path": path }),
                )?;
                if self.project.is_none() {
                    self.populate(&name, &path)?;
                }
                self.project = Some((name, path));
            }
            "chat" => {
                let (project, _) = self.project()?.clone();
                let (chat, _) = self.runtime.block_on(self.client.start_chat(
                    "claude:fake".into(),
                    project.clone(),
                    None,
                ))?;
                self.call(
                    "openTab",
                    json!({ "project": project, "kind": "agent", "target": chat }),
                )?;
                if let Ok(name) = step.text(0) {
                    self.call("renameAgent", json!({ "agentId": chat, "name": name }))?;
                }
            }
            "open" => {
                let (project, path) = self.project()?.clone();
                self.call(
                    "openTab",
                    json!({ "project": project, "kind": "files", "target": path.join(step.text(0)?) }),
                )?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Claude Code's own folder for a project's sessions: every character of its path that is not
/// an ASCII letter or digit turned into `-`.
fn claude_project_dir(root: &Path, cwd: &Path) -> PathBuf {
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    root.join("projects").join(name)
}

fn session_id(seed: usize) -> String {
    let n = 0x5ee1_0000_0000_0000u64 + seed as u64;
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        n >> 32,
        (n >> 16) & 0xffff,
        seed,
        seed,
        n
    )
}

/// Claude's usage answer, with each window's `resetsInMinutes` turned into when it resets.
fn usage(world: &Value) -> Value {
    let mut usage = world.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    fn stamp(window: &mut Value, now: u64) {
        if let Some(minutes) = window["resetsInMinutes"].as_u64() {
            window["resets_at"] = json!(now + minutes * 60);
        }
    }
    if let Some(limits) = usage["rate_limits"].as_object_mut() {
        for (_, window) in limits.iter_mut() {
            match window.as_array_mut() {
                Some(windows) => windows.iter_mut().for_each(|window| stamp(window, now)),
                None => stamp(window, now),
            }
        }
    }
    usage
}

/// `value` with a leading `~/` in a string taken as the home folder.
fn expand_home(value: &Value) -> Value {
    match value.as_str().and_then(|text| text.strip_prefix("~/")) {
        Some(rest) => json!(sikemux_process::paths::home()
            .map_or_else(|| PathBuf::from(rest), |home| home.join(rest))),
        None => value.clone(),
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let group = -(self.core.id() as i32);
        unsafe { libc::kill(group, libc::SIGTERM) };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if matches!(self.core.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe { libc::kill(group, libc::SIGKILL) };
        let _ = self.core.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

enum Sink {
    Video {
        ffmpeg: Child,
        input: Option<ChildStdin>,
        partial: PathBuf,
        out: PathBuf,
    },
    Stills {
        frames: Vec<u64>,
    },
}

/// Where steps that last go, a little at a time each frame.
enum Motion {
    Cursor {
        from: (f32, f32),
        control: (f32, f32),
        to: (f32, f32),
    },
    Camera {
        from: View,
        to: View,
    },
    Press {
        from: f32,
        to: f32,
    },
    Pinch {
        factor: f32,
    },
    Pan {
        dx: f32,
        dy: f32,
    },
    End,
}

struct Tween {
    id: u64,
    start: f64,
    duration: f64,
    motion: Motion,
    began: bool,
    done: f64,
}

#[derive(Debug)]
struct Finished;

impl std::fmt::Display for Finished {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("every still is taken")
    }
}

impl std::error::Error for Finished {}

struct Director {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    stage: (f32, f32),
    origin: (f32, f32),
    pictures_a_frame: usize,
    ticks: u64,
    waited_ms: f64,
    clock: PathBuf,
    recording: bool,
    recorded: u64,
    pointer: Pointer,
    held: bool,
    sent: Option<(f32, f32)>,
    forced: Option<Shape>,
    view: View,
    end: f32,
    tweens: Vec<Tween>,
    next_id: u64,
    lens: Option<u64>,
    compositor: Compositor,
    output: (u32, u32),
    sink: Sink,
    still: Option<PathBuf>,
    stills: PathBuf,
    grid: bool,
    events: Vec<Value>,
    seed: u32,
    began: Instant,
    /// Time spent drawing the app, reading its pictures back, framing them and encoding, for
    /// the progress line.
    spent: [Duration; 4],
}

fn hand_ease(p: f64) -> f64 {
    p * p * p * (10. + p * (6. * p - 15.))
}

fn out_ease(p: f64) -> f64 {
    1. - (1. - p).powi(3)
}

fn in_out(p: f64) -> f64 {
    if p < 0.5 {
        4. * p * p * p
    } else {
        1. - (-2. * p + 2.).powi(3) / 2.
    }
}

/// The camera's curve, CSS's cubic-bezier(0.2, 0.8, 0.2, 1): it starts fast and settles gently.
fn lens_ease(p: f64) -> f64 {
    let along = |t: f64, a: f64, b: f64| {
        3. * a * t * (1. - t).powi(2) + 3. * b * t * t * (1. - t) + t.powi(3)
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..30 {
        let mid = (low + high) / 2.;
        if along(mid, 0.2, 0.2) < p {
            low = mid;
        } else {
            high = mid;
        }
    }
    along((low + high) / 2., 0.8, 1.)
}

impl Director {
    fn now(&self) -> f64 {
        self.ticks as f64 / (FPS * self.pictures_a_frame as f64)
    }

    fn random(&mut self) -> f64 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.seed as f64 / u32::MAX as f64
    }

    fn mark(&mut self, kind: &str, detail: Option<Value>) {
        if !self.recording {
            return;
        }
        let at = (self.recorded as f64 / FPS * 1000.).round() / 1000.;
        let mut event = json!({ "kind": kind, "at": at });
        if let Some(detail) = detail {
            event["detail"] = detail;
        }
        self.events.push(event);
    }

    fn tween(&mut self, duration: f64, motion: Motion) -> u64 {
        self.next_id += 1;
        let step = 1. / (FPS * self.pictures_a_frame as f64);
        self.tweens.push(Tween {
            id: self.next_id,
            start: self.now(),
            duration: duration.max(step),
            motion,
            began: false,
            done: 0.,
        });
        self.next_id
    }

    fn until(&mut self, id: u64) -> anyhow::Result<()> {
        while self.tweens.iter().any(|tween| tween.id == id) {
            self.frame()?;
        }
        Ok(())
    }

    fn wait(&mut self, seconds: f64) -> anyhow::Result<()> {
        for _ in 0..(seconds * FPS).round() as u64 {
            self.frame()?;
        }
        Ok(())
    }

    fn dispatch(&mut self, event: PlatformInput) -> anyhow::Result<()> {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .map_err(|error| anyhow!("{error}"))
    }

    fn position(&self) -> gpui::Point<gpui::Pixels> {
        point(px(self.pointer.x), px(self.pointer.y))
    }

    fn write_clock(&self) {
        let ms = self.waited_ms + self.now() * 1000.;
        let fresh = self.clock.with_extension("next");
        if std::fs::write(&fresh, format!("{ms:.1}")).is_ok() {
            let _ = std::fs::rename(&fresh, &self.clock);
        }
    }

    fn apply(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let mut events = Vec::new();
        let position = self.position();
        let mut finished = Vec::new();
        for tween in &mut self.tweens {
            let p = ((now - tween.start) / tween.duration).clamp(0., 1.);
            let phase = if !tween.began {
                TouchPhase::Started
            } else if p >= 1. {
                TouchPhase::Ended
            } else {
                TouchPhase::Moved
            };
            match &tween.motion {
                Motion::Cursor { from, control, to } => {
                    let e = hand_ease(p) as f32;
                    self.pointer.x =
                        (1. - e).powi(2) * from.0 + 2. * (1. - e) * e * control.0 + e * e * to.0;
                    self.pointer.y =
                        (1. - e).powi(2) * from.1 + 2. * (1. - e) * e * control.1 + e * e * to.1;
                }
                Motion::Camera { from, to } => {
                    let e = lens_ease(p) as f32;
                    let width = from.width * (to.width / from.width).powf(e);
                    let along = if (to.width - from.width).abs() > 1. {
                        (from.width - width) / (from.width - to.width)
                    } else {
                        e
                    };
                    self.view = View {
                        x: from.x + (to.x - from.x) * along,
                        y: from.y + (to.y - from.y) * along,
                        width,
                    };
                }
                Motion::Press { from, to } => {
                    self.pointer.press = from + (to - from) * out_ease(p) as f32;
                }
                Motion::Pinch { factor } => {
                    let e = in_out(p);
                    let by = factor.powf((e - tween.done) as f32);
                    tween.done = e;
                    events.push(PlatformInput::Pinch(PinchEvent {
                        position,
                        delta: by - 1.,
                        modifiers: Modifiers::default(),
                        phase,
                    }));
                }
                Motion::Pan { dx, dy } => {
                    let e = in_out(p);
                    let part = (e - tween.done) as f32;
                    tween.done = e;
                    events.push(PlatformInput::Scroll(ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(dx * part), px(dy * part))),
                        modifiers: Modifiers::default(),
                        touch_phase: phase,
                        source: ScrollSource::Pointer,
                    }));
                }
                Motion::End => {
                    self.end = p as f32;
                    self.pointer.opacity = (1. - p as f32 * 2.5).max(0.);
                }
            }
            tween.began = true;
            if p >= 1. {
                finished.push(tween.id);
            }
        }
        self.tweens.retain(|tween| !finished.contains(&tween.id));
        if self.lens.is_some_and(|lens| finished.contains(&lens)) {
            self.lens = None;
        }
        self.send_pointer()?;
        for event in events {
            self.dispatch(event)?;
        }
        Ok(())
    }

    fn send_pointer(&mut self) -> anyhow::Result<()> {
        let now = (self.pointer.x, self.pointer.y);
        if self
            .sent
            .is_some_and(|sent| (sent.0 - now.0).abs() < 0.25 && (sent.1 - now.1).abs() < 0.25)
        {
            return Ok(());
        }
        self.sent = Some(now);
        let held = self.held.then_some(MouseButton::Left);
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: self.position(),
            pressed_button: held,
            modifiers: Modifiers::default(),
        }))?;
        if self.forced.is_none() {
            let at = self.position();
            self.pointer.shape = self
                .cx
                .update_window(self.window, |_, window, _| {
                    gpui_base::test_support::snapshots(window)
                        .iter()
                        .filter(|element| element.visible() && element.bounds().contains(&at))
                        .filter_map(|element| {
                            let shape = match element.role()? {
                                Role::Button
                                | Role::Link
                                | Role::Tab
                                | Role::MenuItem
                                | Role::CheckBox
                                | Role::RadioButton
                                | Role::Switch
                                | Role::TreeItem
                                | Role::ListBoxOption => Shape::Hand,
                                Role::TextInput | Role::MultilineTextInput | Role::SearchInput => {
                                    Shape::Beam
                                }
                                _ => return None,
                            };
                            let size = element.bounds().size;
                            Some((f32::from(size.width) * f32::from(size.height), shape))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map_or(Shape::Arrow, |(_, shape)| shape)
                })
                .map_err(|error| anyhow!("{error}"))?;
        }
        Ok(())
    }

    fn pump(&mut self) {
        self.cx.update(|cx| cx.set_reduce_motion(false));
        self.cx.run_until_parked();
    }

    fn frame(&mut self) -> anyhow::Result<()> {
        let wanted = self.recording
            && (self.still.is_some()
                || match &self.sink {
                    Sink::Video { .. } => true,
                    Sink::Stills { frames } => frames.contains(&self.recorded),
                });
        let step = Duration::from_secs_f64(1. / (FPS * self.pictures_a_frame as f64));
        for _ in 0..self.pictures_a_frame {
            let drawing = Instant::now();
            self.ticks += 1;
            self.write_clock();
            self.apply()?;
            self.cx.advance_clock(step);
            self.pump();
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.simulate_next_frame(cx);
                })
                .map_err(|error| anyhow!("{error}"))?;
            self.pump();
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .map_err(|error| anyhow!("{error}"))?;
            self.spent[0] += drawing.elapsed();
            if wanted {
                let reading = Instant::now();
                let shot = self.cx.capture_screenshot(self.window)?;
                self.spent[1] += reading.elapsed();
                let framing = Instant::now();
                self.compositor.add(Picture {
                    window: &shot,
                    view: self.view,
                    pointer: Some(self.pointer),
                    end: self.end,
                    grid: self.grid,
                });
                self.spent[2] += framing.elapsed();
            }
        }
        if !self.recording {
            return Ok(());
        }
        if wanted {
            let (width, height) = self.output;
            let frame = self.compositor.take().to_vec();
            let still = self.still.take().or_else(|| match &self.sink {
                Sink::Stills { frames } if frames.contains(&self.recorded) => Some(
                    self.stills
                        .join(format!("still-{:.2}.png", self.recorded as f64 / FPS)),
                ),
                _ => None,
            });
            if let Some(path) = still {
                image::RgbaImage::from_raw(width, height, frame.clone())
                    .ok_or_else(|| anyhow!("frame size"))?
                    .save(&path)?;
                println!("film: saved {}", path.display());
            }
            if let Sink::Video {
                input: Some(input), ..
            } = &mut self.sink
            {
                let encoding = Instant::now();
                input.write_all(&frame)?;
                self.spent[3] += encoding.elapsed();
            }
        }
        self.recorded += 1;
        if self.recorded.is_multiple_of(60) {
            let each = |spent: Duration| spent.as_secs_f64() * 1000. / self.recorded as f64;
            println!(
                "film: {:.1} s filmed, {:.0} ms a frame: app {:.0}, reading {:.0}, framing {:.0}, encoding {:.0}",
                self.recorded as f64 / FPS,
                each(self.began.elapsed()),
                each(self.spent[0]),
                each(self.spent[1]),
                each(self.spent[2]),
                each(self.spent[3]),
            );
        }
        if let Sink::Stills { frames } = &self.sink {
            if frames.iter().all(|frame| *frame < self.recorded) {
                return Err(Finished.into());
            }
        }
        Ok(())
    }
}

/// A box on screen: left, top, width and height.
type Rect = (f32, f32, f32, f32);

impl Director {
    fn place(&mut self, step: &Step, at: usize) -> anyhow::Result<((f32, f32), Option<Rect>)> {
        let word = step
            .words
            .get(at)
            .ok_or_else(|| anyhow!("line {}: {} needs a place", step.line, step.verb))?
            .clone();
        if word.quoted {
            let label = word.text.clone();
            let found = self
                .cx
                .update_window(self.window, |_, window, _| {
                    gpui_base::test_support::snapshots(window)
                        .iter()
                        .filter(|element| {
                            element.visible() && element.label() == Some(label.as_str())
                        })
                        .map(|element| element.bounds())
                        .min_by(|a, b| {
                            (f32::from(a.size.width) * f32::from(a.size.height))
                                .total_cmp(&(f32::from(b.size.width) * f32::from(b.size.height)))
                        })
                })
                .map_err(|error| anyhow!("{error}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "line {}: nothing labelled {:?} is shown",
                        step.line,
                        word.text
                    )
                })?;
            let rect = (
                f32::from(found.origin.x),
                f32::from(found.origin.y),
                f32::from(found.size.width),
                f32::from(found.size.height),
            );
            return Ok(((rect.0 + rect.2 / 2., rect.1 + rect.3 / 2.), Some(rect)));
        }
        match step.numbers(at).as_deref() {
            Some([x, y]) => Ok(((*x, *y), None)),
            Some([x, y, w, h]) => Ok(((x + w / 2., y + h / 2.), Some((*x, *y, *w, *h)))),
            _ => bail!("line {}: {:?} is not a place", step.line, word.text),
        }
    }

    fn glide(
        &mut self,
        to: (f32, f32),
        duration: Option<f64>,
        background: bool,
    ) -> anyhow::Result<()> {
        let from = (self.pointer.x, self.pointer.y);
        let distance = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        if distance < 1. {
            return Ok(());
        }
        let duration =
            duration.unwrap_or((280. + 210. * (1. + distance as f64 / 70.).log2()) / 1000.);
        let bend = if self.random() < 0.5 { -1. } else { 1. } * (distance * 0.08).min(60.);
        let control = (
            (from.0 + to.0) / 2. + (to.1 - from.1) / distance * bend,
            (from.1 + to.1) / 2. - (to.0 - from.0) / distance * bend,
        );
        let id = self.tween(duration, Motion::Cursor { from, control, to });
        if !background {
            self.until(id)?;
        }
        Ok(())
    }

    fn press_to(&mut self, to: f32, duration: f64) -> anyhow::Result<()> {
        let from = self.pointer.press;
        let id = self.tween(duration, Motion::Press { from, to });
        self.until(id)
    }

    fn button(&mut self, down: bool, button: MouseButton, count: usize) -> anyhow::Result<()> {
        let position = self.position();
        let modifiers = Modifiers::default();
        self.dispatch(if down {
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: count,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: count,
            })
        })
    }

    fn click(&mut self, button: MouseButton, count: usize) -> anyhow::Result<()> {
        self.mark("click", None);
        self.press_to(1., 0.07)?;
        for at in 1..=count {
            self.button(true, button, at)?;
            self.wait(0.08)?;
            self.button(false, button, at)?;
            if at < count {
                self.wait(0.05)?;
            }
        }
        self.press_to(0., 0.13)?;
        self.wait(0.16)
    }

    fn zoom(&mut self, to: View, duration: f64, background: bool) -> anyhow::Result<()> {
        let change = self.view.width / to.width;
        if change > 1.15 {
            self.mark(
                "zoom-in",
                Some(json!({ "by": (change * 100.).round() / 100. })),
            );
        } else if change < 1. / 1.15 {
            self.mark(
                "zoom-out",
                Some(json!({ "by": (100. / change).round() / 100. })),
            );
        }
        if let Some(lens) = self.lens.take() {
            self.tweens.retain(|tween| tween.id != lens);
        }
        let id = self.tween(
            duration,
            Motion::Camera {
                from: self.view,
                to,
            },
        );
        self.lens = Some(id);
        if !background {
            self.until(id)?;
        }
        Ok(())
    }

    /// The camera framing `centre` (window points) `scale` times closer, kept inside the stage.
    fn view_at(&self, centre: (f32, f32), scale: f32) -> View {
        let (sw, sh) = self.stage;
        let width = sw / scale.clamp(1., 5.);
        let height = width * sh / sw;
        let (cx, cy) = (self.origin.0 + centre.0, self.origin.1 + centre.1);
        View {
            x: (cx - width / 2.).clamp(0., sw - width),
            y: (cy - height / 2.).clamp(0., sh - height),
            width,
        }
    }

    fn type_text(&mut self, text: &str, per_second: f64) -> anyhow::Result<()> {
        for letter in text.chars() {
            self.mark("key", None);
            let keystroke = match letter {
                ' ' => Keystroke {
                    modifiers: Modifiers::default(),
                    key: "space".into(),
                    key_char: Some(" ".into()),
                },
                _ => Keystroke {
                    modifiers: Modifiers {
                        shift: letter.is_uppercase(),
                        ..Modifiers::default()
                    },
                    key: letter.to_lowercase().to_string(),
                    key_char: Some(letter.to_string()),
                },
            };
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx);
                })
                .map_err(|error| anyhow!("{error}"))?;
            let gap = (0.55 + self.random() * 0.9) / per_second;
            self.wait(if letter == ' ' { gap * 1.3 } else { gap })?;
        }
        Ok(())
    }

    /// Lets the core and the window catch up for `seconds` of real time, unfilmed.
    fn settle(&mut self, seconds: f64) -> anyhow::Result<()> {
        let started = Instant::now();
        let step = Duration::from_millis(50);
        while started.elapsed() < Duration::from_secs_f64(seconds) {
            self.pump();
            std::thread::sleep(step);
            self.waited_ms += 50.;
            self.write_clock();
            self.cx.advance_clock(step);
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .map_err(|error| anyhow!("{error}"))?;
        }
        Ok(())
    }

    fn run(&mut self, step: &Step) -> anyhow::Result<()> {
        let seconds = |at: usize| step.number(at).map(f64::from);
        let background = step.background;
        match step.verb.as_str() {
            "stage" | "window" | "replies" => {}
            "settle" => self.settle(seconds(0).unwrap_or(3.))?,
            "hold" => {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs_f64(seconds(0).unwrap_or(1.)) {
                    self.pump();
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            "record" => self.recording = true,
            "wait" => self.wait(seconds(0).unwrap_or(1.))?,
            "cursor" => {
                let (to, _) = self.place(step, 0)?;
                (self.pointer.x, self.pointer.y) = to;
            }
            "move" => {
                let (to, _) = self.place(step, 0)?;
                self.glide(to, seconds(1), background)?;
            }
            "click" | "dclick" | "rclick" => {
                if !step.words.is_empty() {
                    let (to, _) = self.place(step, 0)?;
                    self.glide(to, seconds(1), false)?;
                    self.wait(0.09)?;
                }
                let button = if step.verb == "rclick" {
                    MouseButton::Right
                } else {
                    MouseButton::Left
                };
                self.click(button, if step.verb == "dclick" { 2 } else { 1 })?;
            }
            "press" => {
                self.press_to(1., 0.07)?;
                self.held = true;
                self.button(true, MouseButton::Left, 1)?;
                self.wait(0.08)?;
            }
            "release" => {
                self.button(false, MouseButton::Left, 1)?;
                self.held = false;
                self.press_to(0., 0.13)?;
            }
            "drag" => {
                let (to, _) = self.place(step, 0)?;
                self.mark("drag", None);
                self.press_to(1., 0.07)?;
                self.held = true;
                self.button(true, MouseButton::Left, 1)?;
                self.wait(0.1)?;
                self.glide(to, seconds(1), false)?;
                self.wait(0.08)?;
                self.button(false, MouseButton::Left, 1)?;
                self.held = false;
                self.press_to(0., 0.13)?;
            }
            "type" => {
                let text = step.text(0)?.to_owned();
                self.type_text(&text, seconds(1).unwrap_or(15.))?;
            }
            "key" => {
                let name = step.text(0)?.to_owned();
                self.mark(if name == "enter" { "send" } else { "key" }, None);
                self.cx
                    .update_window(self.window, |_, window, cx| -> anyhow::Result<()> {
                        let keystroke = Keystroke::parse_with_os(&name, cx.platform_info().os)?;
                        window.dispatch_keystroke(keystroke, cx);
                        Ok(())
                    })
                    .map_err(|error| anyhow!("{error}"))??;
                self.wait(0.2)?;
            }
            "zoom" => {
                if step.text(0)? == "out" {
                    let full = View {
                        x: 0.,
                        y: 0.,
                        width: self.stage.0,
                    };
                    self.zoom(full, seconds(1).unwrap_or(0.7), background)?;
                } else {
                    let (centre, rect) = self.place(step, 0)?;
                    let (view, duration) =
                        match (rect, step.words.first().is_some_and(|word| word.quoted)) {
                            (Some(rect), false) => {
                                let pad = 48.;
                                let fit = (self.stage.0 / (rect.2 + pad * 2.))
                                    .min(self.stage.1 / (rect.3 + pad * 2.));
                                (self.view_at(centre, fit), seconds(1))
                            }
                            _ => (
                                self.view_at(centre, step.number(1).unwrap_or(1.6)),
                                seconds(2),
                            ),
                        };
                    self.zoom(view, duration.unwrap_or(0.7), background)?;
                }
            }
            "pinch" => {
                let factor = step.number(0).unwrap_or(1.5);
                self.mark("pinch", Some(json!({ "by": factor })));
                let id = self.tween(seconds(1).unwrap_or(0.9), Motion::Pinch { factor });
                if !background {
                    self.until(id)?;
                }
            }
            "pan" | "scroll" => {
                let (dx, dy) = if step.verb == "scroll" {
                    (0., step.number(0).unwrap_or(-300.))
                } else {
                    match step.numbers(0).as_deref() {
                        Some([dx, dy]) => (*dx, *dy),
                        _ => bail!("line {}: pan DX,DY", step.line),
                    }
                };
                self.mark("pan", None);
                let id = self.tween(seconds(1).unwrap_or(0.8), Motion::Pan { dx, dy });
                if !background {
                    self.until(id)?;
                }
            }
            "shape" => {
                self.forced = match step.text(0)? {
                    "arrow" => Some(Shape::Arrow),
                    "hand" => Some(Shape::Hand),
                    "beam" => Some(Shape::Beam),
                    _ => None,
                };
                if let Some(shape) = self.forced {
                    self.pointer.shape = shape;
                } else {
                    self.sent = None;
                }
            }
            "mark" => self.mark(step.text(0)?, None),
            "still" => {
                self.still = Some(self.stills.join(format!("{}.png", step.text(0)?)));
                self.frame()?;
            }
            "end" => {
                self.mark("end-card", None);
                let full = View {
                    x: 0.,
                    y: 0.,
                    width: self.stage.0,
                };
                if self.view != full {
                    self.zoom(full, 0.9, true)?;
                }
                let id = self.tween(0.9, Motion::End);
                self.until(id)?;
                self.wait(seconds(0).unwrap_or(2.2))?;
            }
            other => bail!("line {}: no step called {other}", step.line),
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let args = args()?;
    let scene_text = std::fs::read_to_string(&args.scene)
        .with_context(|| format!("reading {}", args.scene.display()))?;
    let steps = read_scene(&scene_text)?;
    let header = |verb: &str| steps.iter().find(|step| step.verb == verb);
    let pair = |verb: &str, fallback: (f32, f32)| {
        header(verb)
            .and_then(|step| Some((step.number(0)?, step.number(1)?)))
            .unwrap_or(fallback)
    };
    let stage = pair("stage", (1760., 990.));
    let window_size = pair("window", (1520., 900.));
    let replies = header("replies")
        .map(|step| step.text(0).map(|file| args.scene.with_file_name(file)))
        .transpose()?;
    std::fs::create_dir_all(&args.work)?;
    let clock = args.work.join("clock");
    std::fs::write(&clock, "0")?;
    let mut world = World::start(&args, replies, &clock)?;

    let bench = Rc::new(RefCell::new(None));
    let kept = bench.clone();
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    let link = core::start(core::Home::at(world.socket.clone()));
    let window = cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.set_reduce_motion(false);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(window_size.0), px(window_size.1)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| workbench::Workbench::new(link, window, cx));
                *kept.borrow_mut() = Some(view.clone());
                cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
            },
        )
    })?;
    // A GIF plays only in the window in front, as on the Mac.
    cx.update_window(window.into(), |_, window, _| window.activate_window())?;
    if let Some(bench) = bench.borrow().clone() {
        cx.update(|cx| {
            let _ = bench.read(cx).asks.send(core::Ask::RegisterWindow);
        });
    }
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    let density = cx.capture_screenshot(window.into())?.width() as f32 / window_size.0;

    let output_height = args.height;
    let output_width = ((output_height as f32 * stage.0 / stage.1 / 2.).round() * 2.) as u32;
    let compositor = Compositor::new(Setup {
        stage,
        window: window_size,
        density,
        output: (output_width, output_height),
        wallpaper: args.wallpaper.as_deref(),
        lockup: args.lockup.as_deref(),
    });
    let origin = compositor.window_origin();
    let (sink, stills) = match &args.stills {
        Some(times) => {
            std::fs::create_dir_all(&args.out)?;
            let frames = times
                .iter()
                .map(|seconds| (seconds * FPS).round() as u64)
                .collect();
            (Sink::Stills { frames }, args.out.clone())
        }
        None => {
            let partial = args.out.with_extension("partial.mp4");
            let mut ffmpeg = sikemux_process::user_environment::command("ffmpeg")
                .args([
                    "-y",
                    "-loglevel",
                    "error",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                ])
                .args(["-s", &format!("{output_width}x{output_height}")])
                .args(["-framerate", &FPS.to_string(), "-i", "-"])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    if args.draft { "veryfast" } else { "slow" },
                ])
                .args([
                    "-crf",
                    if args.draft { "23" } else { "14" },
                    "-pix_fmt",
                    "yuv420p",
                ])
                .args(["-profile:v", "high", "-movflags", "+faststart"])
                .arg(&partial)
                .stdin(Stdio::piped())
                .spawn()
                .context("starting ffmpeg")?;
            let input = ffmpeg.stdin.take();
            let folder = args
                .out
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            (
                Sink::Video {
                    ffmpeg,
                    input,
                    partial,
                    out: args.out.clone(),
                },
                folder,
            )
        }
    };
    let mut director = Director {
        cx,
        window: window.into(),
        stage,
        origin,
        pictures_a_frame: if args.blur { 2 } else { 1 },
        ticks: 0,
        waited_ms: 0.,
        clock,
        recording: false,
        recorded: 0,
        pointer: Pointer {
            x: window_size.0 * 0.75,
            y: window_size.1 * 0.8,
            press: 0.,
            opacity: 1.,
            shape: Shape::Arrow,
        },
        held: false,
        sent: None,
        forced: None,
        view: View {
            x: 0.,
            y: 0.,
            width: stage.0,
        },
        end: 0.,
        tweens: Vec::new(),
        next_id: 0,
        lens: None,
        compositor,
        output: (output_width, output_height),
        sink,
        still: None,
        stills,
        grid: args.grid,
        events: Vec::new(),
        seed: 7,
        began: Instant::now(),
        spent: [Duration::ZERO; 4],
    };
    let mut played = Ok(());
    for step in &steps {
        let taken = match world.take(step) {
            Ok(taken) => taken,
            Err(error) => {
                played = Err(error.context(format!("line {}", step.line)));
                break;
            }
        };
        if taken {
            continue;
        }
        if let Err(error) = director.run(step) {
            played = Err(error);
            break;
        }
    }
    while director.recording && !director.tweens.is_empty() && played.is_ok() {
        played = director.frame();
    }
    drop(world);
    let played = match played {
        Err(error) if error.is::<Finished>() => Ok(()),
        other => other,
    };
    if let Sink::Video {
        mut ffmpeg,
        input,
        partial,
        out,
    } = std::mem::replace(&mut director.sink, Sink::Stills { frames: Vec::new() })
    {
        drop(input);
        let encoded = ffmpeg.wait()?;
        if played.is_ok() && encoded.success() {
            std::fs::rename(&partial, &out)?;
            println!(
                "film: wrote {} ({:.1} s)",
                out.display(),
                director.recorded as f64 / FPS
            );
        }
    }
    if let Some(events) = &args.events {
        std::fs::write(
            events,
            serde_json::to_string_pretty(&json!({
                "fps": FPS,
                "seconds": director.recorded as f64 / FPS,
                "events": director.events,
            }))?,
        )?;
    }
    played
}
