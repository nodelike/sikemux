//! The throwaway world a film runs in: a core, projects, chats and activity to show.

use super::*;
use sikemux_core::sessions::claude::claude_project_dir;

/// The throwaway core the film talks to, started by this process so a dev core trusts it.
pub(super) struct World {
    pub(super) core: Child,
    pub(super) runtime: tokio::runtime::Runtime,
    pub(super) client: CoreClient,
    pub(super) _events: Box<dyn std::any::Any>,
    pub(super) socket: PathBuf,
    pub(super) work: PathBuf,
    pub(super) home: PathBuf,
    pub(super) scenes: PathBuf,
    pub(super) project: Option<(String, PathBuf)>,
    pub(super) chat: Option<String>,
    /// The chats the scene opened, by the name it gave each.
    named: std::collections::HashMap<String, String>,
    pub(super) claude: PathBuf,
    pub(super) world: Value,
}

impl World {
    pub(super) fn start(
        args: &Args,
        replies: Option<PathBuf>,
        clock: &Path,
    ) -> anyhow::Result<Self> {
        let work = &args.work;
        let home = work.join("home");
        let claude = work.join("claude");
        for folder in [&home, &claude, &work.join("data")] {
            std::fs::create_dir_all(folder)?;
        }
        let bin = work.join("bin");
        std::fs::create_dir_all(&bin)?;
        // ssh reads the config of the account's own home, not HOME, so the world's hosts are
        // reached by a stand-in that says it connected and then waits: nothing leaves the Mac.
        let ssh = bin.join("ssh");
        std::fs::write(
            &ssh,
            "#!/bin/sh\nfor host; do :; done\nprintf '\\033]6973;ssh;connected\\007'\nprintf 'Last login: Mon Nov  2 09:12:44 2026\\r\\n%s ~ $ ' \"$host\"\nexec /bin/sleep 86400\n",
        )?;
        std::fs::set_permissions(&ssh, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        std::fs::write(
            home.join(".zshrc"),
            format!(
                "path=({} $path)\nPROMPT='%F{{6}}%1~%f %F{{5}}❯%f '\n",
                bin.display()
            ),
        )?;
        let ssh: String = args.world["sshHosts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|host| {
                Some(format!(
                    "Host {}\n  HostName {}\n  User {}\n\n",
                    host["name"].as_str()?,
                    host["hostName"].as_str()?,
                    host["user"].as_str().unwrap_or("deploy"),
                ))
            })
            .collect();
        std::fs::create_dir_all(home.join(".ssh"))?;
        std::fs::write(home.join(".ssh/config"), ssh)?;
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
        let path = command
            .get_envs()
            .find(|(name, _)| *name == "PATH")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default();
        command.env("PATH", format!("{}:{path}", bin.display()));
        command
            .arg("core")
            .arg("--socket")
            .arg(&socket)
            .arg("--data-dir")
            .arg(work.join("data"))
            .arg("--cli-endpoint")
            .arg(work.join("cli.json"))
            .env("HOME", &home)
            // Agents' commands find the core by the endpoint it hands them only when it lies in
            // their config folder, so the core keeps the throwaway home's.
            .env_remove(sikemux_process::vars::CONFIG_DIR)
            .env("SIKEMUX_CORE_SOCKET", &socket)
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .env("FAKE_CLAUDE_MODEL", SEEDED_MODEL)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0);
        if let Some(replies) = &replies {
            command.env("FAKE_CLAUDE_SCRIPT", replies);
        }
        // A clip paces the replies by its own time; snaps take the clock on the wall, so the
        // durations the chat shows are real ones.
        if !args.stills.as_ref().is_some_and(Vec::is_empty) {
            command.env("FAKE_CLAUDE_CLOCK", clock);
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
            chat: None,
            named: std::collections::HashMap::new(),
            claude,
            world: args.world.clone(),
        })
    }

    /// A year of work in `path` for the activity calendar: busier on weekdays and lately. The
    /// core makes its tables on the first read, so one comes first.
    pub(super) fn seed_activity(&self, path: &Path) -> anyhow::Result<()> {
        self.call("activitySummary", json!({ "utcOffsetMinutes": 0 }))?;
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
        let mut sql = String::from("BEGIN;\n");
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
                    "INSERT INTO turns (started_ms, ended_ms, agent, project) \
                     VALUES ({started}, {}, 'claude', '{place}');\n",
                    started + (busy * 900_000.) as i64 + 60_000,
                );
            }
            sql += &format!(
                "INSERT INTO tokens (at_ms, agent, project, input, output, cache_read, cache_write, model) \
                 VALUES ({at}, 'claude', '{place}', {}, {}, {}, {}, '{SEEDED_MODEL}');\n",
                (busy * 900_000.) as i64,
                (busy * 300_000.) as i64,
                (busy * 6_000_000.) as i64,
                (busy * 1_500_000.) as i64,
            );
            for commit in 0..(busy * 6.) as i64 {
                sql += &format!(
                    "INSERT OR IGNORE INTO commits (sha, at_ms, project, agent) \
                     VALUES ('{back:x}{commit:x}', {}, '{place}', 'claude');\n",
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
    pub(super) fn populate(&self, project: &str, path: &Path) -> anyhow::Result<()> {
        let world = &self.world;
        let path = &path.canonicalize()?;
        for other in world["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            self.add_project(other, None)?;
        }
        let spaces: Vec<&Value> = world["spaces"].as_array().into_iter().flatten().collect();
        if !spaces.is_empty() {
            let infos: Vec<Value> = spaces
                .iter()
                .map(|space| json!({ "id": space["id"], "name": space["name"], "icon": space["icon"] }))
                .collect();
            self.call("setSpaces", json!({ "spaces": infos }))?;
        }
        for space in &spaces {
            for project in space["projects"].as_array().into_iter().flatten() {
                self.call(
                    "updateProject",
                    json!({ "id": project, "patch": { "space": space["id"] } }),
                )?;
            }
        }
        for host in world["sshHosts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|host| host["name"].as_str())
        {
            self.call("openSshTerminal", json!({ "host": host }))?;
        }
        let transcripts = claude_project_dir(&self.claude, &path.to_string_lossy());
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
                agent["project"].as_str().unwrap_or(project).to_owned(),
                None,
            ))?;
            if let Some(name) = agent["name"].as_str() {
                self.rename(&chat, name)?;
                named.insert(name.to_owned(), chat.clone());
            }
            let prompt = match agent["state"].as_str() {
                Some("working") => Some("hold 3600000".to_owned()),
                Some("done") => Some(agent["title"].as_str().unwrap_or("done").to_owned()),
                _ => None,
            };
            if let Some(prompt) = prompt {
                self.runtime.block_on(self.client.ask(ask::AcpPrompt {
                    agent_id: chat.clone(),
                    message_id: None,
                    text: prompt,
                    paths: Vec::new(),
                    context: Vec::new(),
                }))?;
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
            if let Err(error) = self.open_tab(project, path, tab) {
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

    /// Adds a git project called `name`, a copy of `from` or else a folder holding only a README.
    pub(super) fn add_project(&self, name: &str, from: Option<&Path>) -> anyhow::Result<PathBuf> {
        let path = self.work.join("projects").join(name);
        std::fs::create_dir_all(path.parent().unwrap_or(&self.work))?;
        match from {
            Some(from) => {
                let copied = sikemux_process::user_environment::command("cp")
                    .arg("-R")
                    .arg(from)
                    .arg(&path)
                    .status()?;
                if !copied.success() {
                    bail!("could not copy {}", from.display());
                }
            }
            None => {
                std::fs::create_dir_all(&path)?;
                std::fs::write(path.join("README.md"), format!("# {name}\n"))?;
            }
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
        let mut copied: Vec<String> = std::fs::read_dir(&path)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != ".git")
            .collect();
        copied.sort();
        let mut add = vec!["add", "--"];
        add.extend(copied.iter().map(String::as_str));
        git(&add)?;
        git(&["commit", "-q", "-m", "Start"])?;
        self.call(
            "addProject",
            json!({ "id": name, "name": name, "path": path }),
        )?;
        Ok(path)
    }

    /// Opens a terminal, the Git tab, or a file of the project.
    pub(super) fn open_tab(&self, project: &str, path: &Path, tab: &str) -> anyhow::Result<Value> {
        match tab {
            "terminal" => self.call("newTerminal", json!({ "project": project })),
            "git" => self.call(
                "openTab",
                json!({ "project": project, "kind": "git", "target": path }),
            ),
            file => self.call(
                "openTab",
                json!({ "project": project, "kind": "files", "target": path.join(file) }),
            ),
        }
    }

    pub(super) fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        Ok(self.runtime.block_on(self.client.call(method, params))?)
    }

    /// Names a chat that just started, once the core has written it in its book of names.
    fn rename(&self, chat: &str, name: &str) -> anyhow::Result<()> {
        let params = json!({ "agentId": chat, "name": name });
        for _ in 0..50 {
            match self.call("renameAgent", params.clone()) {
                Err(error) if error.to_string().contains("not named that agent yet") => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                done => return done.map(|_| ()),
            }
        }
        self.call("renameAgent", params).map(|_| ())
    }

    pub(super) fn project(&self) -> anyhow::Result<&(String, PathBuf)> {
        self.project
            .as_ref()
            .ok_or_else(|| anyhow!("add a project first"))
    }

    /// Does `step` when it is the core's to do.
    pub(super) fn take(&mut self, step: &Step) -> anyhow::Result<bool> {
        match step.verb.as_str() {
            "project" => {
                let name = step.text(0)?.to_owned();
                let path = self.add_project(&name, Some(&self.scenes.join(step.text(1)?)))?;
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
                self.call(
                    "moveTab",
                    json!({ "project": project, "tab": format!("agent:{chat}"), "to": 0 }),
                )?;
                if let Ok(name) = step.text(0) {
                    self.rename(&chat, name)?;
                    self.named.insert(name.to_owned(), chat.clone());
                }
                self.chat = Some(chat);
            }
            "lead" => {
                let chat = (self.chat.clone()).ok_or_else(|| anyhow!("open a chat first"))?;
                let lead = (self.named.get(step.text(0)?).cloned())
                    .ok_or_else(|| anyhow!("no chat named {}", step.text(0).unwrap_or_default()))?;
                self.call(
                    "updateAgent",
                    json!({ "agentId": chat, "patch": { "lead": lead } }),
                )?;
            }
            "front" => {
                let (project, _) = self.project()?.clone();
                let chat = (self.named.get(step.text(0)?).cloned())
                    .ok_or_else(|| anyhow!("no chat named {}", step.text(0).unwrap_or_default()))?;
                self.call(
                    "openTab",
                    json!({ "project": project, "kind": "agent", "target": chat }),
                )?;
                self.chat = Some(chat);
            }
            "title" => {
                let chat = self
                    .chat
                    .clone()
                    .ok_or_else(|| anyhow!("open a chat first"))?;
                self.call(
                    "updateAgent",
                    json!({ "agentId": chat, "patch": { "title": step.text(0)?, "renamed": true } }),
                )?;
            }
            "trigger" => {
                let chat = self
                    .chat
                    .clone()
                    .ok_or_else(|| anyhow!("open a chat first"))?;
                let (_, path) = self.project()?.clone();
                let title = step.text(0)?;
                let every: u64 = step.text(1)?.parse()?;
                let params = json!({ "title": title, "prompt": title, "every": every });
                sikemux_core::cli::client::attempt(
                    &self.work.join("cli.json"),
                    &path.to_string_lossy(),
                    Some(&chat),
                    "trigger.add",
                    &params,
                    Duration::from_secs(10),
                )
                .map_err(|failure| match failure {
                    sikemux_core::cli::client::Failure::NotSent(error)
                    | sikemux_core::cli::client::Failure::Sent(error) => anyhow!(error),
                })?;
            }
            "say" => {
                let chat = self
                    .chat
                    .clone()
                    .ok_or_else(|| anyhow!("open a chat first"))?;
                self.runtime.block_on(self.client.ask(ask::AcpPrompt {
                    agent_id: chat,
                    message_id: None,
                    text: step.text(0)?.to_owned(),
                    paths: Vec::new(),
                    context: Vec::new(),
                }))?;
            }
            "auto" => {
                let (_, path) = self.project()?.clone();
                let slots: u32 = step.text(0)?.parse()?;
                let ceiling: u32 = step.text(1)?.parse()?;
                self.call(
                    "boardSettings",
                    json!({ "project": path, "auto": true, "slots": slots, "spendCeiling": ceiling }),
                )?;
            }
            "pause" => {
                let trigger: u64 = step.text(0)?.parse()?;
                self.call("holdTrigger", json!({ "trigger": trigger, "paused": true }))?;
            }
            "prefer" => {
                let key = step.text(0)?;
                let value = match step.text(1) {
                    Err(_) => expand_home(&self.world["preferences"][key]),
                    Ok("null") => Value::Null,
                    Ok(text) => serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
                };
                self.call("setPreference", json!({ "key": key, "value": value }))?;
            }
            "patch" => {
                let (_, path) = self.project()?.clone();
                let applied = sikemux_process::user_environment::command("git")
                    .arg("apply")
                    .arg(self.scenes.join(step.text(0)?))
                    .current_dir(&path)
                    .status()?;
                if !applied.success() {
                    bail!("could not apply {}", step.text(0)?);
                }
            }
            "page" => {
                let pages = self.work.join("data").join(sikemux_core::pages::FOLDER);
                std::fs::create_dir_all(&pages)?;
                std::fs::copy(
                    self.scenes.join(step.text(1)?),
                    pages.join(format!("{}.html", step.text(0)?)),
                )?;
            }
            "open" => {
                let (project, path) = self.project()?.clone();
                self.open_tab(&project, &path, step.text(0)?)?;
            }
            "core" => {
                let (_, path) = self.project()?.clone();
                let params = step.text(1)?.replace("$project", &path.to_string_lossy());
                self.call(step.text(0)?, serde_json::from_str(&params)?)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

pub(super) fn session_id(seed: usize) -> String {
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
pub(super) fn usage(world: &Value) -> Value {
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

/// `value` with a leading `~/` in a string taken as the home folder. A world names files in
/// one person's home, so a run elsewhere is told when one is not there.
pub(super) fn expand_home(value: &Value) -> Value {
    match value.as_str().filter(|text| text.starts_with("~/")) {
        Some(text) => {
            let path = sikemux_process::paths::expand_home(text);
            if !path.exists() {
                eprintln!(
                    "film: {} is not on this machine, so it goes without",
                    path.display()
                );
            }
            json!(path)
        }
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
