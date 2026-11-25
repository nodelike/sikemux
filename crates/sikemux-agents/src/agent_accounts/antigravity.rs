//! The Antigravity accounts added beside the first one. `agy` keeps the first in the macOS
//! keychain, which holds one; an added one is a folder of its own, where `agy` keeps the
//! sign-in as a file when told to. Every use of an added account checks in agy's own log that
//! it did, so no added account ever runs on the first one's sign-in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sikemux_process::command::AsyncCommand;
use sikemux_process::paths::account_dir;

use super::folders::MARKER;
use crate::launch::named_account;

const FILE_STORE: (&str, &str) = (sikemux_process::vars::ANTIGRAVITY_FILE_STORE, "1");
const FILE_STORE_IN_USE: &str = "Using file-based token storage";
/// What points agy at a Gemini key rather than a sign-in.
const KEY: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_GEMINI_BASE_URL"];
const DATA: &str = "antigravity-cli";
const TOKEN: &str = "antigravity-oauth-token";
const HOME: &str = "home";
const LOG_FOLDER: &str = "chat-log";
const SIGN_IN_LOG: &str = "acp-login-";
/// agy writes to its log as it opens a session, so the line is there or about to be.
const LOG_WAIT: Duration = Duration::from_secs(3);

pub const WRONG_STORE: &str = "Antigravity did not confirm it uses this account's own sign-in, \
     so Sikemux stopped it rather than use your other account. Updating Sikemux may help.";

/// An added account's folder, one Sikemux made.
#[derive(Clone, Debug)]
pub struct Added {
    folder: PathBuf,
}

/// The added account in `config_path`; none for the first account.
pub fn added(config_path: Option<&str>) -> Result<Option<Added>, String> {
    match config_path.map(str::trim).filter(|path| !path.is_empty()) {
        None => Ok(None),
        Some(path) => Added::at(&account_dir(path)).map(Some),
    }
}

/// The added account a launch's environment names; none for the first account.
pub fn added_in(env: &BTreeMap<String, String>) -> Result<Option<Added>, String> {
    added(named_account("antigravity", env))
}

impl Added {
    fn at(folder: &Path) -> Result<Self, String> {
        match folder.join(MARKER).is_file() {
            true => Ok(Self {
                folder: folder.to_owned(),
            }),
            false => Err(
                "This Antigravity account's folder is not one Sikemux made, so it is not used. \
                 Remove the account and add it again."
                    .into(),
            ),
        }
    }

    fn token(&self) -> PathBuf {
        self.folder.join(DATA).join(TOKEN)
    }

    /// Whether agy keeps a sign-in in the folder; whether Google still takes it is agy's to say.
    pub fn signed_in(&self) -> bool {
        self.token()
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
    }

    /// Makes the folder and what is in it the person's alone.
    pub fn keep_private(&self) -> std::io::Result<()> {
        crate::os::make_private_folder(&self.folder)?;
        crate::os::make_private_folder(&self.folder.join(DATA))?;
        match self.signed_in() {
            true => crate::os::make_private_file(&self.token()),
            false => Ok(()),
        }
    }

    pub fn forget_sign_in(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.token()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            removed => removed,
        }
    }

    pub fn sign_in_args(&self) -> Vec<String> {
        let mut args = vec!["acp".to_owned()];
        args.extend(self.folder_args());
        args.push("--login".to_owned());
        args
    }

    fn folder_args(&self) -> [String; 2] {
        [
            "--gemini-dir".to_owned(),
            self.folder.to_string_lossy().into_owned(),
        ]
    }

    /// Starts a chat's agy on this account, or says why it may not be.
    pub fn chat(&self) -> Result<OnAccount, String> {
        if !self.signed_in() {
            return Err("This Antigravity account is not signed in.".into());
        }
        self.keep_private()
            .and_then(|()| std::fs::create_dir_all(self.folder.join(LOG_FOLDER)))
            .and_then(|()| crate::os::make_private_folder(&self.folder.join(LOG_FOLDER)))
            .map_err(|error| format!("Could not use this Antigravity account's folder: {error}"))?;
        let name = format!("{}.log", uuid::Uuid::new_v4());
        Ok(OnAccount {
            account: self.clone(),
            log: self.folder.join(LOG_FOLDER).join(name),
        })
    }

    /// Asks agy a question about this account alone: its home is a folder that leads back to
    /// the account's, for the commands that take no folder of their own.
    pub fn question(&self, command: &mut AsyncCommand) -> Result<Question, String> {
        if !self.signed_in() {
            return Err(super::usage::SIGNED_OUT.into());
        }
        self.keep_private()
            .and_then(|()| self.make_home())
            .map_err(|error| format!("Could not use this Antigravity account's folder: {error}"))?;
        command
            .env("HOME", self.folder.join(HOME))
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "1")
            .envs([FILE_STORE])
            .env_remove(KEY[0])
            .env_remove(KEY[1]);
        Ok(self.asked(None))
    }

    /// The sign-in about to start, whose log tells where agy would keep it.
    pub fn signing_in(&self) -> Question {
        self.asked(Some(SIGN_IN_LOG))
    }

    fn asked(&self, named: Option<&'static str>) -> Question {
        Question {
            account: self.clone(),
            named,
            since: SystemTime::now(),
        }
    }

    fn make_home(&self) -> std::io::Result<()> {
        let home = self.folder.join(HOME);
        std::fs::create_dir_all(&home)?;
        crate::os::make_private_folder(&home)?;
        let link = home.join(".gemini");
        if std::fs::read_link(&link).is_ok_and(|to| to == Path::new("..")) {
            return Ok(());
        }
        crate::os::symlink(Path::new(".."), &link)
    }
}

/// What an agent must not be started with: an added account pays for its own chats, so a
/// Gemini key never stands in for it.
pub fn withheld(provider: &str, env: &BTreeMap<String, String>) -> &'static [&'static str] {
    match provider == "antigravity" && named_account(provider, env).is_some() {
        true => &KEY,
        false => &[],
    }
}

/// Keeps agy on the file store, for any command run with an added account's environment.
pub fn file_store(config_path: Option<&str>) -> Option<(&'static str, &'static str)> {
    config_path
        .is_some_and(|path| !path.trim().is_empty())
        .then_some(FILE_STORE)
}

/// One agy question or sign-in on an added account.
pub struct Question {
    account: Added,
    named: Option<&'static str>,
    since: SystemTime,
}

impl Question {
    /// Whether an agy started since says in its log that it uses the file store.
    pub fn on_file_store(&self) -> bool {
        self.logs().any(|log| says_file_store(&log))
    }

    /// The logs written since the question was asked.
    fn logs(&self) -> impl Iterator<Item = PathBuf> + '_ {
        let logs = std::fs::read_dir(self.account.folder.join(DATA).join("log"));
        (logs.into_iter().flatten().flatten())
            .filter(|log| {
                let name = log.file_name();
                (self.named).is_none_or(|named| name.to_string_lossy().starts_with(named))
                    && (log.metadata().and_then(|meta| meta.modified()))
                        .is_ok_and(|written| written >= self.since)
            })
            .map(|log| log.path())
    }

    /// [`on_file_store`](Self::on_file_store), waiting for agy to write its log.
    pub async fn confirmed(&self) -> bool {
        soon(|| self.on_file_store()).await
    }
}

impl Drop for Question {
    /// agy may have written the sign-in anew, and a sign-in's log holds its link.
    fn drop(&mut self) {
        let _ = self.account.keep_private();
        if self.named.is_some() {
            for log in self.logs() {
                let _ = std::fs::remove_file(log);
            }
        }
    }
}

async fn soon(confirmed: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    loop {
        if confirmed() {
            return true;
        }
        if started.elapsed() > LOG_WAIT {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn says_file_store(log: &Path) -> bool {
    std::fs::read(log).is_ok_and(|said| String::from_utf8_lossy(&said).contains(FILE_STORE_IN_USE))
}

/// A chat's agy on an added account.
pub struct OnAccount {
    account: Added,
    log: PathBuf,
}

impl OnAccount {
    pub fn args(&self) -> Vec<String> {
        let mut args = self.account.folder_args().to_vec();
        args.push("--log-file".into());
        args.push(self.log.to_string_lossy().into_owned());
        args
    }

    pub fn env(&self) -> (String, String) {
        (FILE_STORE.0.to_owned(), FILE_STORE.1.to_owned())
    }

    /// Whether agy's log says it uses the account's own sign-in, once it has opened a session,
    /// and the account was not removed since. The log names the account, so it goes once read.
    pub async fn confirmed(&self) -> bool {
        let own =
            soon(|| says_file_store(&self.log)).await && Added::at(&self.account.folder).is_ok();
        let _ = self.account.keep_private();
        own
    }
}

impl Drop for OnAccount {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.log);
    }
}

/// What a new account's folder starts with, and the shared folders it links to.
pub(crate) fn seed(home: &Path, directory: &Path) -> Result<(), String> {
    if cfg!(windows) {
        return Err("A second Antigravity account is not available on Windows yet".into());
    }
    let made = || {
        for shared in ["conversations", "brain", "annotations", "implicit"] {
            std::fs::create_dir_all(home.join(".gemini").join(DATA).join(shared))?;
        }
        std::fs::create_dir_all(directory.join(DATA))?;
        let account = Added {
            folder: directory.to_owned(),
        };
        account.keep_private()?;
        account.make_home()
    };
    made().map_err(|error: std::io::Error| error.to_string())
}

/// Removes an added account's folder with its sign-in. The chats and settings in it are links,
/// so the first account keeps them.
pub fn remove(config_path: &str) -> Result<(), String> {
    let folder = account_dir(config_path);
    let own = folder.parent() == sikemux_process::paths::home().as_deref()
        && (folder.file_name().and_then(|name| name.to_str()))
            .is_some_and(|name| name.starts_with(".gemini-"))
        && folder.symlink_metadata().is_ok_and(|meta| meta.is_dir())
        && folder.join(MARKER).is_file();
    match own {
        true => std::fs::remove_dir_all(&folder)
            .map_err(|error| format!("Could not remove this account's sign-in: {error}")),
        false => Ok(()),
    }
}
