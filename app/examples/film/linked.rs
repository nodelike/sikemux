//! The film's Sikemux account and its other hosts: a local accounts API that every film core
//! signs in to by code as a person would, and throwaway cores of their own on that account.

use super::*;
use sikemux_agents::sessions::claude::claude_project_dir;
use sikemux_process::command::ChildStdout;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};

const VECTOR: &str = include_str!("../../../server/protocol/vectors/join.json");

/// The accounts API the film's cores sign in to, with the person and the orgs the world's
/// `sikemuxAccount` names (see `server/api/dev/film-account.ts`).
pub(super) struct Account {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    pub(super) api: String,
    /// Where each core listens, by its key, for every config folder that dials them.
    addresses: BTreeMap<String, Vec<String>>,
    folders: Vec<PathBuf>,
}

impl Account {
    pub(super) fn start(work: &Path, world: &Value) -> anyhow::Result<Self> {
        let server = Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/api");
        let mut child = sikemux_process::user_environment::command("node")
            .arg(server.join("dev/film-account.ts"))
            .arg(work.join("account"))
            .arg(world.to_string())
            .current_dir(&server)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(work.join("account.log"))?)
            .spawn()
            .context("starting the film's accounts API with node")?;
        let input = child.stdin.take();
        let mut output = BufReader::new(
            (child.stdout.take()).ok_or_else(|| anyhow!("the accounts API has no output"))?,
        );
        let mut line = String::new();
        output.read_line(&mut line)?;
        let started: Value = serde_json::from_str(&line).with_context(|| {
            format!(
                "the accounts API did not start; see {}",
                work.join("account.log").display()
            )
        })?;
        Ok(Self {
            child,
            input,
            output,
            api: (started.get("api").and_then(Value::as_str))
                .ok_or_else(|| anyhow!("the accounts API named no address"))?
                .to_owned(),
            addresses: BTreeMap::new(),
            folders: vec![sikemux_process::paths::config_dir()
                .ok_or_else(|| anyhow!("the film has no config folder"))?],
        })
    }

    /// Readies a core under `home` to sign in here: it trusts the API's tickets and dials the
    /// film's other cores where they listen, as test cores have no relay to meet through.
    pub(super) fn prepare(&mut self, home: &Path) -> anyhow::Result<()> {
        let config = home.join(".config/sikemux");
        let dev = config.join("dev");
        std::fs::create_dir_all(&dev)?;
        let vector: Value = serde_json::from_str(VECTOR)?;
        let pem = (vector.get("privateKeyPem"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("the join vector has no key"))?;
        std::fs::write(dev.join("join-signing-key.pem"), pem)?;
        self.folders.push(config);
        Ok(())
    }

    /// Signs the core behind `client` in by code, approving the code as the person, and names
    /// it `name` on the account, never this computer's own name.
    pub(super) fn sign_in(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        client: &CoreClient,
        data: &Path,
        name: &str,
    ) -> anyhow::Result<()> {
        let shown = runtime.block_on(client.call("accountSignInByCode", json!({})))?;
        let code = (shown.get("code"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("signing in by code gave no code"))?;
        let input = (self.input.as_mut()).ok_or_else(|| anyhow!("the accounts API stopped"))?;
        writeln!(input, "approve {code}")?;
        let mut line = String::new();
        self.output.read_line(&mut line)?;
        if line.trim() != format!("approved {code}") {
            bail!("the accounts API did not approve {code}: {line}");
        }
        runtime.block_on(client.call("accountSignInFinish", json!({})))?;
        let status = listening(runtime, client)?;
        name_on_account(runtime, client, &status.core_id, data, name)?;
        self.addresses.insert(status.core_id, status.addresses);
        let named = serde_json::to_string(&self.addresses)?;
        for folder in &self.folders {
            std::fs::create_dir_all(folder.join("dev"))?;
            std::fs::write(folder.join("dev/host-addresses.json"), &named)?;
        }
        Ok(())
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}

pub(super) struct Linked {
    pub(super) name: String,
    core: Child,
    client: CoreClient,
    _events: Box<dyn std::any::Any>,
    socket: PathBuf,
    claude: PathBuf,
    work: PathBuf,
    /// Its copy of the film's last project.
    pub(super) project: Option<PathBuf>,
    frozen: bool,
}

/// Waits for the core at `socket` to answer.
pub(super) fn connect(
    runtime: &tokio::runtime::Runtime,
    socket: &Path,
) -> anyhow::Result<(CoreClient, Box<dyn std::any::Any>)> {
    let started = Instant::now();
    loop {
        match runtime.block_on(CoreClient::connect(socket)) {
            Ok((client, events)) => return Ok((client, Box::new(events))),
            Err(error) if started.elapsed() > Duration::from_secs(30) => {
                bail!("the core never answered at {}: {error}", socket.display())
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn listening(
    runtime: &tokio::runtime::Runtime,
    client: &CoreClient,
) -> anyhow::Result<sikemux_wire::protocol::RemoteStatus> {
    let started = Instant::now();
    loop {
        let status = runtime.block_on(client.remote_status())?;
        if !status.addresses.is_empty() && !status.core_id.is_empty() {
            return Ok(status);
        }
        if started.elapsed() > Duration::from_secs(30) {
            bail!("a core's remote access never listened");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Renames the core on the account once its first name check has sent the computer's name,
/// since a rename before that check is taken back.
fn name_on_account(
    runtime: &tokio::runtime::Runtime,
    client: &CoreClient,
    key: &str,
    data: &Path,
    name: &str,
) -> anyhow::Result<()> {
    let started = Instant::now();
    while !data.join("account-host-name").exists() {
        if started.elapsed() > Duration::from_secs(30) {
            bail!("the core signed in as {name} never sent its own name to the account");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    runtime.block_on(client.call("renameAccountDevice", json!({ "key": key, "name": name })))?;
    Ok(())
}

impl World {
    /// Starts host `name` on the film's account, with a copy of the last project.
    pub(super) fn link_host(&mut self, name: &str) -> anyhow::Result<()> {
        let work = self.work.join("hosts").join(name);
        let home = work.join("home");
        let claude = home.join(".claude");
        for folder in [&claude, &work.join("data")] {
            std::fs::create_dir_all(folder)?;
        }
        let account =
            (self.account.as_mut()).ok_or_else(|| anyhow!("the film is not signed in"))?;
        account.prepare(&home)?;
        let api = account.api.clone();
        let socket = work.join("core.sock");
        let log = std::fs::File::create(work.join("core.log"))?;
        let mut command = sikemux_process::user_environment::command(&self.core_program);
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
            .env(sikemux_process::vars::API_URL, api)
            .env_remove(sikemux_process::vars::CONFIG_DIR)
            .env("SIKEMUX_CORE_SOCKET", &socket)
            .env(sikemux_process::vars::AGENT_SEARCH, "path")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        let core = sikemux_process::spawn_tree(&mut command)
            .with_context(|| format!("starting host {name}"))?;
        let (client, events) = connect(&self.runtime, &socket)?;
        let profile = json!({
            "id": "builtin-claude", "provider": "claude", "accent": "#d97757", "name": "Claude",
            "executablePath": self.claude_program, "configPath": claude,
        });
        self.runtime
            .block_on(client.call("setAgentAccounts", json!({ "profiles": [profile] })))?;
        let mut linked = Linked {
            name: name.to_owned(),
            core,
            client,
            _events: events,
            socket,
            claude,
            work,
            project: None,
            frozen: false,
        };
        if let Some((project, path)) = self.project.clone() {
            let copy = linked.work.join("projects").join(&project);
            std::fs::create_dir_all(linked.work.join("projects"))?;
            let copied = sikemux_process::user_environment::command("cp")
                .arg("-R")
                .arg(&path)
                .arg(&copy)
                .status()?;
            if !copied.success() {
                bail!("could not copy {project} to {name}");
            }
            self.runtime.block_on(linked.client.call(
                "addProject",
                json!({ "id": project, "name": project, "path": copy }),
            ))?;
            linked.project = Some(sikemux_process::files::real_path(&copy)?);
        }
        let account =
            (self.account.as_mut()).ok_or_else(|| anyhow!("the film is not signed in"))?;
        account.sign_in(
            &self.runtime,
            &linked.client,
            &linked.work.join("data"),
            name,
        )?;
        self.linked.push(linked);
        Ok(())
    }

    /// Saves a Claude chat titled `title` in the last project, on host `name` or `here`.
    pub(super) fn save_chat_on(&self, name: &str, title: &str) -> anyhow::Result<()> {
        let (claude, path, first) = match name {
            "here" => (
                &self.claude,
                sikemux_process::files::real_path(&self.project()?.1)?,
                500,
            ),
            name => {
                let (at, linked) = (self.linked.iter().enumerate())
                    .find(|(_, linked)| linked.name == name)
                    .ok_or_else(|| anyhow!("no host named {name}"))?;
                let path = (linked.project.clone())
                    .ok_or_else(|| anyhow!("{name} holds no project; add one before the host"))?;
                (&linked.claude, path, 700 + 20 * at)
            }
        };
        let saved = saved_on(claude, &path);
        save_transcript(claude, &path, first + saved, title, 2 + 60 * saved as u64)
    }

    /// Stops host `name` answering, as a computer that went to sleep.
    pub(super) fn freeze(&mut self, name: &str) -> anyhow::Result<()> {
        let linked = (self.linked.iter_mut())
            .find(|linked| linked.name == name)
            .ok_or_else(|| anyhow!("no host named {name}"))?;
        // SAFETY: kill only sends a signal to the process this film started.
        if unsafe { libc::kill(linked.core.id() as i32, libc::SIGSTOP) } != 0 {
            bail!("could not stop {name}");
        }
        linked.frozen = true;
        Ok(())
    }
}

impl Drop for Linked {
    fn drop(&mut self) {
        if self.frozen {
            // SAFETY: as in freeze, a signal to the process this film started.
            unsafe { libc::kill(self.core.id() as i32, libc::SIGCONT) };
        }
        sikemux_process::terminate_tree(self.core.id(), true);
        let _ = self.core.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn saved_on(claude: &Path, path: &Path) -> usize {
    std::fs::read_dir(claude_project_dir(claude, &path.to_string_lossy()))
        .map(Iterator::count)
        .unwrap_or(0)
}

/// Writes the transcript of a chat titled `title` in `path`, last touched `minutes` ago.
pub(super) fn save_transcript(
    claude: &Path,
    path: &Path,
    seed: usize,
    title: &str,
    minutes: u64,
) -> anyhow::Result<()> {
    let transcripts = claude_project_dir(claude, &path.to_string_lossy());
    std::fs::create_dir_all(&transcripts)?;
    let session = session_id(seed);
    let file = transcripts.join(format!("{session}.jsonl"));
    let lines = [
        json!({ "type": "user", "uuid": session_id(seed + 100), "sessionId": session, "cwd": path,
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
    sikemux_process::user_environment::command("touch")
        .args(["-A", &format!("-{:02}{:02}00", minutes / 60, minutes % 60)])
        .arg(&file)
        .status()?;
    Ok(())
}
