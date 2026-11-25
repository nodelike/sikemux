//! The throwaway world a film runs in: a core, projects, chats and activity to show.

use super::*;

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
    /// The folder of the checkout the last `core` call made, for `$checkout`.
    checkout: Option<String>,
    /// The remote each project a scene adds gets, in a scene on a recorded team or one naming an
    /// `origin`; the world's other projects have none.
    pub(super) origin: Option<String>,
    /// The team's other hosts, in a scene where they ask about its tasks.
    teammates: Option<Teammates>,
    pub(super) claude: PathBuf,
    pub(super) world: Value,
    pub(super) core_program: PathBuf,
    pub(super) claude_program: PathBuf,
    pub(super) linked: Vec<Linked>,
    pub(super) account: Option<Account>,
}

impl World {
    pub(super) fn start(
        args: &Args,
        replies: Option<PathBuf>,
        team: Option<(PathBuf, String)>,
        asks: bool,
        clock: &Path,
        mut account: Option<Account>,
        signs_in_at_step: bool,
    ) -> anyhow::Result<Self> {
        let work = &args.work;
        let home = work.join("home");
        let claude = home.join(".claude");
        for folder in [&home, &claude, &work.join("data")] {
            std::fs::create_dir_all(folder)?;
        }
        let bin = work.join("bin");
        std::fs::create_dir_all(&bin)?;
        let stand_in = |name: &str, script: String| stand_in(&bin, name, &script);
        // ssh reads the config of the account's own home, not HOME, so the world's hosts are
        // reached by a stand-in that says it connected and then waits: nothing leaves the Mac.
        stand_in(
            "ssh",
            "#!/bin/sh\nfor host; do :; done\nprintf '\\033]6973;ssh;connected\\007'\nprintf 'Last login: Mon Nov  2 09:12:44 2026\\r\\n%s ~ $ ' \"$host\"\nexec /bin/sleep 86400\n".into(),
        )?;
        stand_in("open", OPEN.into())?;
        // Antigravity and Codex chats run on the stand-in agents built beside the stand-in Claude.
        let fake = |name: &str| args.claude.with_file_name(name).display().to_string();
        let gemini = [
            ("gemini-4-neon-high", "Gemini 4 Neon (High)"),
            ("gemini-4-carbon-high", "Gemini 4 Carbon (High)"),
            ("gemini-3.8-flash-high", "Gemini 3.8 Flash (High)"),
        ];
        let listed: String = gemini
            .iter()
            .map(|(id, name)| format!("{id}\\t{name}\\n"))
            .collect();
        let offered = json!(gemini
            .iter()
            .map(|(id, name)| json!({ "value": id, "name": name }))
            .collect::<Vec<_>>());
        let agy_usage = bin.join("agy-usage.json");
        std::fs::write(
            &agy_usage,
            antigravity_usage(
                args.world
                    .pointer("/antigravity/usage")
                    .unwrap_or(&Value::Null),
            )
            .to_string(),
        )?;
        stand_in(
            "agy",
            format!(
                "#!/bin/sh\n[ \"$1\" = models ] && exec printf '{listed}'\n[ \"$2\" = /usage ] && exec cat '{}'\nFAKE_ACP_MODELS='{offered}' exec '{}' \"$@\"\n",
                agy_usage.display(),
                fake("sikemux-fake-acp-agent")
            ),
        )?;
        stand_in(
            "codex",
            format!("#!/bin/sh\nexec '{}' \"$@\"\n", fake("sikemux-fake-codex")),
        )?;
        std::fs::write(
            home.join(".zshrc"),
            format!(
                "path=({} $path)\nPROMPT='%F{{6}}%1~%f %F{{5}}❯%f '\n",
                bin.display()
            ),
        )?;
        let ssh: String = args
            .world
            .get("sshHosts")
            .and_then(Value::as_array)
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
            usage(args.world.pointer("/account/usage").unwrap_or(&Value::Null)).to_string(),
        )?;
        std::fs::write(claude.join(".claude.json"), signed_in("edon"))?;
        if let Some(account) = &mut account {
            account.prepare(&home)?;
        }
        let socket = work.join("core.sock");
        let _ = std::fs::remove_file(&socket);
        if let Some((folder, _)) = &team {
            seat_team(folder, work, &home, &socket)?;
        }
        let teammates = (team.is_some() && asks)
            .then(|| Teammates::seat(work, &home, &args.claude))
            .transpose()?;
        let log = std::fs::File::create(work.join("core.log"))?;
        let mut command = core_command(&args.core, &bin);
        command
            .arg("core")
            .arg("--socket")
            .arg(&socket)
            .arg("--data-dir")
            .arg(work.join("data"))
            .arg("--cli-endpoint")
            .arg(work.join("cli.json"))
            .arg("--throwaway")
            .env("HOME", &home)
            // Agents' commands find the core by the endpoint it hands them only when it lies in
            // their config folder, so the core keeps the throwaway home's.
            .env_remove(sikemux_process::vars::CONFIG_DIR)
            .env("SIKEMUX_CORE_SOCKET", &socket)
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .env("FAKE_CLAUDE_MODEL", SEEDED_MODEL)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(account) = &account {
            command.env(sikemux_process::vars::API_URL, &account.api);
        }
        if let Some(replies) = &replies {
            command.env("FAKE_CLAUDE_SCRIPT", replies);
            command.env("FAKE_ACP_SCRIPT", replies);
        }
        // A clip paces the replies by its own time; snaps take the clock on the wall, so the
        // durations the chat shows are real ones.
        if !args.stills.as_ref().is_some_and(Vec::is_empty) {
            command.env("FAKE_CLAUDE_CLOCK", clock);
        }
        let core = sikemux_process::spawn_tree(&mut command).context("starting the core")?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (client, events) = connect(&runtime, &socket)?;
        let profile = json!({
            "id": "builtin-claude", "provider": "claude", "accent": "#d97757",
            "name": args.world.pointer("/account/name").and_then(Value::as_str).unwrap_or("Claude"),
            "executablePath": args.claude, "configPath": claude,
        });
        runtime.block_on(client.call("setAgentAccounts", json!({ "profiles": [profile] })))?;
        runtime.block_on(client.call(
            "selectAgentAccount",
            json!({ "provider": "claude", "id": "builtin-claude" }),
        ))?;
        let version = client.hello().build.version.clone();
        let mut preferences = vec![
            ("onboardingComplete".to_owned(), json!(true)),
            ("lastSeenVersion".to_owned(), json!(version)),
        ];
        if let Some(kept) = args.world.get("preferences").and_then(Value::as_object) {
            preferences.extend(
                kept.iter()
                    .map(|(key, value)| (key.clone(), expand_home(value))),
            );
        }
        for (method, params) in preferences
            .iter()
            .flat_map(|(key, value)| setting(key, value))
        {
            runtime.block_on(client.call(method, params))?;
        }
        if let Some(face) = args.world.get("faceStyle").filter(|face| face.is_object()) {
            runtime.block_on(client.call("setFaceStyle", face.clone()))?;
        }
        let mut world = Self {
            core,
            runtime,
            client,
            _events: events,
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
            checkout: None,
            origin: team.map(|(_, origin)| origin),
            teammates,
            claude,
            world: args.world.clone(),
            core_program: args.core.clone(),
            claude_program: args.claude.clone(),
            linked: Vec::new(),
            account,
        };
        if !signs_in_at_step {
            world.sign_in()?;
        }
        Ok(world)
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
        let path = &sikemux_process::files::real_path(path)?;
        for other in world["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            self.add_project(other, None, None)?;
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
        let mut pinned = Vec::new();
        for (at, chat) in world["recent"].as_array().into_iter().flatten().enumerate() {
            let title = chat["title"].as_str().unwrap_or("Chat");
            save_transcript(&self.claude, path, at, title, 20 + at as u64 * 25)?;
            if chat["pinned"] == true {
                pinned.push(format!("claude:{}", session_id(at)));
            }
        }
        self.seed_activity(path)?;
        let mut named = std::collections::HashMap::new();
        let agents: Vec<&Value> = world["agents"].as_array().into_iter().flatten().collect();
        for agent in &agents {
            let (chat, _) = self.runtime.block_on(self.client.start_chat(
                "claude:builtin-claude".into(),
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
                if let Some(fields) = patch.as_object_mut() {
                    fields.insert("lead".to_owned(), json!(lead));
                }
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
    pub(super) fn add_project(
        &self,
        name: &str,
        from: Option<&Path>,
        remote: Option<&str>,
    ) -> anyhow::Result<PathBuf> {
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
        if let Some(remote) = remote {
            git(&["remote", "add", "origin", remote])?;
        }
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

    /// Signs in one more Claude account, `name`, with `used` percent of its week gone and the
    /// week refilling in `days`.
    fn add_account(&self, name: &str, used: u64, days: u64) -> anyhow::Result<()> {
        let id = format!("film-{}", name.to_lowercase().replace(' ', "-"));
        let folder = self.home.join(format!(".claude-{id}"));
        std::fs::create_dir_all(&folder)?;
        std::fs::write(
            folder.join("fake-account.json"),
            json!({ "loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max" })
                .to_string(),
        )?;
        let limits = json!({
            "rate_limits_available": true,
            "subscription_type": "max",
            "rate_limits": {
                "five_hour": { "utilization": used / 4, "resetsInMinutes": 120 },
                "seven_day": { "utilization": used, "resetsInMinutes": days * 24 * 60 },
            },
        });
        std::fs::write(folder.join("fake-usage.json"), usage(&limits).to_string())?;
        std::fs::write(folder.join(".claude.json"), signed_in(&id))?;
        let book = self.call("agentAccounts", json!({}))?;
        let mut profiles = (book.get("profiles").and_then(Value::as_array))
            .cloned()
            .unwrap_or_default();
        let program = profiles
            .iter()
            .find(|profile| profile.get("provider") == Some(&json!("claude")))
            .and_then(|profile| profile.get("executablePath").cloned())
            .ok_or_else(|| anyhow!("the world has no Claude account to copy"))?;
        profiles.push(json!({
            "id": id, "provider": "claude", "accent": "#e0663a", "name": name,
            "executablePath": program, "configPath": folder,
        }));
        self.call("setAgentAccounts", json!({ "profiles": profiles }))?;
        Ok(())
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

    /// Signs the film's core in to the world's accounts API, as the host its sikemuxAccount names.
    fn sign_in(&mut self) -> anyhow::Result<()> {
        let Some(account) = &mut self.account else {
            return Ok(());
        };
        let name = (self.world.pointer("/sikemuxAccount/host"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("the world's sikemuxAccount names no host"))?;
        account.sign_in(
            &self.runtime,
            &self.client,
            &self.work.join("data"),
            name,
            None,
        )
    }

    fn sign_out_antigravity(&self) -> anyhow::Result<()> {
        let bin = self.work.join("bin");
        std::fs::rename(bin.join("agy"), bin.join("agy-signed-in"))?;
        stand_in(
            &bin,
            "agy",
            &format!("#!/bin/sh\nbin='{}'\n{SIGNED_OUT}", bin.display()),
        )?;
        self.call("forgetAgentStatus", json!({}))?;
        Ok(())
    }

    /// Does `step` when it is the core's to do.
    pub(super) fn chat_named(&self, name: &str) -> anyhow::Result<String> {
        (self.named.get(name).cloned()).ok_or_else(|| anyhow!("no chat named {name}"))
    }

    pub(super) fn take(&mut self, step: &Step) -> anyhow::Result<bool> {
        match step.verb.as_str() {
            "project" => {
                let name = step.text(0)?.to_owned();
                let from = self.scenes.join(step.text(1)?);
                let remote = step.text(2).ok().or(self.origin.as_deref());
                let path = self.add_project(&name, Some(&from), remote)?;
                if self.project.is_none() {
                    self.populate(&name, &path)?;
                }
                self.project = Some((name, path));
            }
            "signed-in" => self.sign_in()?,
            "signed-out" => self.sign_out_antigravity()?,
            "host" => self.link_host(step.text(0)?, step.text(1).ok())?,
            "joins" => self.join_device(
                step.text(0)?,
                step.text(1)?,
                step.text(2)?,
                step.text(3).is_ok(),
            )?,
            "saved" => self.save_chat_on(step.text(0)?, step.text(1)?)?,
            "asleep" => self.freeze(step.text(0)?)?,
            "orgrole" => self.org_change("role", step.text(0)?, step.text(1)?)?,
            "orgplan" => self.org_change("plan", step.text(0)?, step.text(1)?)?,
            "orgkey" => self.org_key(step.text(0)?, step.text(1)?, step.text(2)?)?,
            "orgpolicy" => self.org_change("policy", step.text(0)?, step.text(1)?)?,
            "orgshare" => self.org_share(step.text(0)?, step.text(1).unwrap_or("here"))?,
            "orgmember" => self.org_change("member", step.text(0)?, step.text(1)?)?,
            "orgmove" => self.org_move(step.text(0)?, step.text(1)?)?,
            "orgholds" => self.org_holds(step.text(0)?, step.text(1)?)?,
            "chat" => {
                let (project, _) = self.project()?.clone();
                let launcher = step.text(1).unwrap_or("claude:builtin-claude").to_owned();
                let model = step.text(2).ok().map(str::to_owned);
                let (chat, _) = self.runtime.block_on(self.client.start_chat(
                    launcher,
                    project.clone(),
                    model,
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
            "account" => self.add_account(
                step.text(0)?,
                step.text(1)?.parse()?,
                step.text(2)?.parse()?,
            )?,
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
            "tags" => {
                let chat = (self.chat.clone()).ok_or_else(|| anyhow!("open a chat first"))?;
                let tags: Vec<&str> = (0..).map_while(|at| step.text(at).ok()).collect();
                self.call(
                    "updateAgent",
                    json!({ "agentId": chat, "patch": { "tags": tags } }),
                )?;
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
                let title = step.text(0)?;
                let every: u64 = step.text(1)?.parse()?;
                self.call(
                    "addTrigger",
                    json!({
                        "title": title,
                        "prompt": title,
                        "when": { "kind": "every", "seconds": every },
                        "wakes": { "kind": "chat", "agent": chat },
                    }),
                )?;
            }
            "say" => {
                let chat = self
                    .chat
                    .clone()
                    .ok_or_else(|| anyhow!("open a chat first"))?;
                self.runtime.block_on(
                    self.client.ask(ask::AcpPrompt {
                        agent_id: chat,
                        message_id: None,
                        text: step.text(0)?.to_owned(),
                        paths: (1..)
                            .map_while(|at| step.text(at).ok())
                            .map(|path| self.scenes.join(path).to_string_lossy().into_owned())
                            .collect(),
                        context: Vec::new(),
                    }),
                )?;
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
                    Err(_) => expand_home(
                        self.world
                            .get("preferences")
                            .and_then(|preferences| preferences.get(key))
                            .unwrap_or(&Value::Null),
                    ),
                    Ok("null") => Value::Null,
                    Ok(text) => {
                        let projects = self.work.join("projects");
                        let projects =
                            sikemux_process::files::real_path(&projects).unwrap_or(projects);
                        let text = text.replace("$projects", &projects.to_string_lossy());
                        serde_json::from_str(&text).unwrap_or_else(|_| json!(text))
                    }
                };
                for (method, params) in setting(key, &value) {
                    self.call(method, params)?;
                }
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
                let pages = self.work.join("data").join(sikemux_base::pages::FOLDER);
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
            "ask" => {
                let (teammates, origin) = (self.teammates.as_ref())
                    .zip(self.origin.as_deref())
                    .context("only a scene on a recorded team has teammates to ask")?;
                let task = step.text(2)?.parse()?;
                let said = self.runtime.block_on(teammates.ask(
                    &self.client,
                    origin,
                    step.text(0)?,
                    step.text(1)?,
                    task,
                    step.text(3)?,
                ))?;
                eprintln!(
                    "film: {} asked about task {task} and heard {said}",
                    step.text(0)?
                );
            }
            "core" => {
                let (_, path) = self.project()?.clone();
                let path = sikemux_process::files::real_path(&path).unwrap_or(path);
                let params = match step.text(1)? {
                    file if file.ends_with(".json") => {
                        std::fs::read_to_string(self.scenes.join(file))?
                    }
                    text => text.to_owned(),
                };
                let params = params
                    .replace("$project", &path.to_string_lossy())
                    .replace("$chat", self.chat.as_deref().unwrap_or_default())
                    .replace("$checkout", self.checkout.as_deref().unwrap_or_default());
                let answer = self.call(step.text(0)?, serde_json::from_str(&params)?)?;
                if let (Ok("openChief"), Some(chief)) =
                    (step.text(0), answer.get("agentId").and_then(Value::as_str))
                {
                    self.named.insert("Chief".to_owned(), chief.to_owned());
                }
                if let Some(folder) = answer.pointer("/worktree/path").and_then(Value::as_str) {
                    self.checkout = Some(folder.to_owned());
                }
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
/// Copies a recorded team account into the world before its core starts, with its teammates'
/// boards heard just now. A dev core keeps its host key in a file named for its socket's path,
/// and trusts the recorded account only beside that key.
fn seat_team(folder: &Path, work: &Path, home: &Path, socket: &Path) -> anyhow::Result<()> {
    let copied = sikemux_process::user_environment::command("cp")
        .arg("-R")
        .arg(folder.join("."))
        .arg(work)
        .status()?;
    if !copied.success() {
        bail!("could not copy {}", folder.display());
    }
    let boards_file = work.join("data/teammate-boards.json");
    let mut copies: Value = serde_json::from_slice(&std::fs::read(&boards_file)?)?;
    let boards = copies
        .get_mut("boards")
        .and_then(Value::as_array_mut)
        .context("the recorded boards have no list")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let latest = boards
        .iter()
        .filter_map(|board| board.get("heard")?.as_u64())
        .max()
        .unwrap_or(now);
    let shift = |at: &mut Value| {
        if let Some(ms) = at.as_u64() {
            *at = json!(ms + now.saturating_sub(latest));
        }
    };
    for board in boards {
        board.get_mut("heard").map(shift);
        let tasks = board
            .pointer_mut("/board/tasks")
            .and_then(Value::as_array_mut);
        for task in tasks.into_iter().flatten() {
            task.get_mut("since").map(shift);
            task.pointer_mut("/fact/at").map(shift);
        }
    }
    std::fs::write(&boards_file, serde_json::to_vec(&copies)?)?;
    let mut shasum = sikemux_process::user_environment::command("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    shasum
        .stdin
        .take()
        .context("shasum took no input")?
        .write_all(socket.as_os_str().as_encoded_bytes())?;
    let digest = String::from_utf8(shasum.wait_with_output()?.stdout)?;
    let keys = home.join(".config/sikemux/secrets/sikemux-host-dev");
    std::fs::create_dir_all(&keys)?;
    std::fs::rename(
        work.join("host"),
        keys.join(format!(
            "core.sock-{}",
            digest.get(..16).unwrap_or_default()
        )),
    )?;
    Ok(())
}

#[expect(
    clippy::indexing_slicing,
    reason = "indexing adds a missing rate_limits key as null, which the fake usage file keeps"
)]
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

/// What `agy -p /usage` prints, with each bucket's `resetsInMinutes` turned into when it resets.
fn antigravity_usage(world: &Value) -> Value {
    let mut usage = world.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let groups = usage
        .pointer_mut("/command/data/groups")
        .and_then(Value::as_array_mut);
    for group in groups.into_iter().flatten() {
        let buckets = group.get_mut("buckets").and_then(Value::as_array_mut);
        for bucket in buckets.into_iter().flatten() {
            if let Some(minutes) = bucket["resetsInMinutes"].as_u64() {
                bucket["reset_time"] = json!(utc(now + minutes * 60));
            }
        }
    }
    usage
}

/// `seconds` since 1970 as an RFC 3339 time in UTC, by Howard Hinnant's days-to-civil.
fn utc(seconds: u64) -> String {
    let (days, of_day) = (seconds / 86_400, seconds % 86_400);
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let of_era = shifted % 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
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
        sikemux_process::terminate_tree(self.core.id(), false);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if matches!(self.core.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        sikemux_process::terminate_tree(self.core.id(), true);
        let _ = self.core.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// The calls that set one preference; the person's own themes are files the core saves.
fn setting(key: &str, value: &Value) -> Vec<(&'static str, Value)> {
    match (key, value) {
        ("customThemes", Value::Array(themes)) => themes
            .iter()
            .map(|theme| ("saveTheme", json!({ "theme": theme })))
            .collect(),
        _ => vec![("setPreference", json!({ "key": key, "value": value }))],
    }
}

/// The `.claude.json` a Claude login leaves, which the core reads to know an account is signed in.
fn signed_in(account: &str) -> String {
    json!({ "oauthAccount": { "accountUuid": account } }).to_string()
}

/// A signed-out agent CLI, such as agy, opens its sign-in page with `open`, which would land
/// in the person's own browser: the world's `open` goes nowhere.
const SIGNED_OUT: &str = r#"
if [ "$1 $2" = 'acp --login' ]; then
  printf 'Open this page to sign in to your Google account:\n\n  https://accounts.example.com/o/oauth2/v2/auth?response_type=code&client_id=stand-in&redirect_uri=https%%3A%%2F%%2Fexample.com%%2Fcallback&scope=openid+email+profile\n\nPaste the code it gives you: '
  read -r code
  [ "$code" = 0000 ] && echo 'Login closed.' && exit 0
  [ "$code" = 4821 ] || { echo 'That code was not accepted.'; exit 1; }
  : > "$bin/agy-account"
  echo 'Signed in.'
  exit 0
fi
[ "$1" = models ] && [ ! -e "$bin/agy-account" ] && exit 1
exec "$bin/agy-signed-in" "$@"
"#;

const OPEN: &str = "#!/bin/sh\nexit 0\n";

fn stand_in(bin: &Path, name: &str, script: &str) -> anyhow::Result<()> {
    let path = bin.join(name);
    std::fs::write(&path, script)?;
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    Ok(())
}

/// The command that starts one of the world's cores, which finds the stand-ins in `bin` first.
/// Folders in the person's own home leave its PATH, so the agents they installed for
/// themselves do not show up in the world as installed.
pub(super) fn core_command(program: &Path, bin: &Path) -> sikemux_process::command::Command {
    let mut command = sikemux_process::user_environment::command(program);
    let path = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let shared = std::env::split_paths(&path)
        .filter(|folder| !home.as_ref().is_some_and(|home| folder.starts_with(home)));
    if let Ok(path) = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(shared)) {
        command.env("PATH", path);
    }
    // Set by a Sikemux agent's own shell, they point past the world's .zshrc.
    command.env_remove("ZDOTDIR");
    command.env_remove(sikemux_process::vars::USER_ZDOTDIR);
    // The GitHub connector signs in with them, and the pictures would show the person's account.
    for token in ["GH_TOKEN", "GITHUB_TOKEN"] {
        command.env_remove(token);
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// On 22 Nov a shots run's core started the person's signed-in agy under its throwaway
    /// home, and agy's `open` put Google's sign-in page in the person's browser.
    #[test]
    fn a_world_core_opens_nothing_in_the_persons_browser() {
        let bin = std::env::temp_dir().join(format!("film-open-{}", std::process::id()));
        std::fs::create_dir_all(&bin).expect("bin");
        stand_in(&bin, "open", OPEN).expect("stand-in");

        let found = core_command(Path::new("/bin/sh"), &bin)
            .args(["-c", "command -v open"])
            .output()
            .expect("sh");

        std::fs::remove_dir_all(&bin).ok();
        assert_eq!(
            String::from_utf8_lossy(&found.stdout).trim(),
            bin.join("open").display().to_string()
        );
    }
}
