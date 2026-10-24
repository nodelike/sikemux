//! The hosts in the person's SSH config, each with how its terminals are
//! reaching it, and a watch on the config that tells clients when it changes.

use std::fs;
use std::path::PathBuf;
use std::sync::Weak;

use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use sikemux_pty::shell_protocol::SshLink;

use super::changed::Topic;
use super::Core;

/// How a host's terminals reach it, the best of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Reach {
    /// No terminal is open to it.
    Idle,
    Failed,
    Connecting,
    Ok,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshHost {
    /// What `ssh` is given, and what the host is called.
    alias: String,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    /// The config's line, from 0, whose `Host` names it.
    line: usize,
    status: Reach,
}

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh/config"))
}

/// The hosts in the config, wildcard patterns left out since nothing
/// connects to them. `Include` is not followed.
pub(crate) fn hosts(core: &Core) -> Vec<SshHost> {
    watch(core);
    let content = config_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    let reach = |alias: &str| {
        core.ssh_links()
            .into_iter()
            .filter(|(host, _)| host == alias)
            .map(|(_, link)| match link {
                Some(SshLink::Connected) => Reach::Ok,
                Some(SshLink::Connecting) => Reach::Connecting,
                Some(SshLink::Disconnected) => Reach::Failed,
                None => Reach::Idle,
            })
            .max()
            .unwrap_or(Reach::Idle)
    };
    let mut hosts = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let (mut hostname, mut user, mut port, mut at) = (None, None, None, 0);
    let mut flush =
        |aliases: &[String], hostname: &Option<String>, user: &Option<String>, port, line| {
            for alias in aliases.iter().filter(|alias| !alias.contains(['*', '?'])) {
                hosts.push(SshHost {
                    alias: alias.clone(),
                    hostname: hostname.clone(),
                    user: user.clone(),
                    port,
                    line,
                    status: reach(alias),
                });
            }
        };
    for (number, line) in content.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut split = line.splitn(2, |c: char| c.is_whitespace() || c == '=');
        let key = split.next().unwrap_or("").to_ascii_lowercase();
        let value = split
            .next()
            .unwrap_or("")
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .trim();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "host" => {
                flush(&aliases, &hostname, &user, port, at);
                aliases = value.split_whitespace().map(String::from).collect();
                (hostname, user, port, at) = (None, None, None, number);
            }
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()),
            "port" => port = value.parse().ok(),
            _ => {}
        }
    }
    flush(&aliases, &hostname, &user, port, at);
    hosts
}

/// Starts watching the config once, the first time a client lists the hosts.
fn watch(core: &Core) {
    let Some(folder) = config_path().and_then(|path| path.parent().map(PathBuf::from)) else {
        return;
    };
    let weak: Weak<Core> = core.me.clone();
    core.ssh_watch.get_or_init(|| {
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let touches_config = event.is_ok_and(|event| {
                    event
                        .paths
                        .iter()
                        .any(|path| path.file_name().is_some_and(|name| name == "config"))
                });
                if let (true, Some(core)) = (touches_config, weak.upgrade()) {
                    core.changes.changed(Topic::Ssh, None, None);
                }
            })
            .ok()?;
        watcher.watch(&folder, RecursiveMode::NonRecursive).ok()?;
        Some(watcher)
    });
}

/// The SSH config's path, creating an empty one the editor can open when
/// there is none. An existing config is never changed. Runs on a blocking
/// thread.
pub(crate) fn config_file() -> Result<String, String> {
    let path = config_path().ok_or("HOME is not set")?;
    let folder = path.parent().ok_or("the SSH config has no folder")?;
    if !folder.exists() {
        fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        fs::set_permissions(folder, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    if !path.exists() {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&path).map_err(|error| error.to_string())?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// A terminal to an SSH host started, ended, or changed how it reaches it.
pub(crate) fn moved(core: &Core) {
    core.changes.changed(Topic::Ssh, None, None);
    core.changes.changed(Topic::Hosts, None, None);
}

/// How many times a dropped connection is retried before the terminal goes
/// back to the local shell.
const RETRIES: &str = "5";

/// The command an ssh terminal runs, through the person's own shell: ssh to
/// `alias`, retried a few times when the connection drops, then back to the
/// local shell. It reports where the connection stands for the rail.
pub(crate) fn startup(alias: &str) -> String {
    if cfg!(windows) {
        windows_startup(alias)
    } else {
        format!("/bin/sh -c {}", portable_quote(&unix_loop(alias)))
    }
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

/// Reads the same in POSIX shells and in fish, where `\\` and `\'` are
/// escapes even inside single quotes.
fn portable_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for character in value.chars() {
        match character {
            '\'' => quoted.push_str(r#"'"'"'"#),
            '\\' => quoted.push_str(r#"'"\\"'"#),
            character => quoted.push(character),
        }
    }
    quoted.push('\'');
    quoted
}

fn unix_loop(alias: &str) -> String {
    const LOOP: &str = r#"sikemux_ssh_link() { printf '\033]6973;ssh;%s\007' "$1"; }; trap 'sikemux_ssh_link disconnected' EXIT; trap 'exit 130' INT TERM HUP; sikemux_ssh_link connecting; sikemux_ssh_retries=0; while :; do command ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o PermitLocalCommand=yes -o "LocalCommand=/usr/bin/printf '\033]6973;ssh;connected\007'" @HOST@; sikemux_ssh_status=$?; stty sane 2>/dev/null || true; printf '\033[0m\033[?25h\033[?1l\033>\033[?2004l\033[?1000l\033[?1002l\033[?1003l\033[?1006l'; if [ "$sikemux_ssh_status" -eq 0 ]; then break; fi; if [ "$sikemux_ssh_status" -eq 130 ]; then printf '\r\nSSH reconnect cancelled. Back at your local shell.\r\n'; break; fi; if [ "$sikemux_ssh_retries" -ge @RETRIES@ ]; then printf '\r\nSSH could not reconnect after @RETRIES@ retries. Back at your local shell.\r\n'; break; fi; sikemux_ssh_retries=$((sikemux_ssh_retries + 1)); printf '\r\nSSH connection lost. Retrying (%s/@RETRIES@) in 3 seconds… Press Ctrl-C to stop.\r\n' "$sikemux_ssh_retries"; sikemux_ssh_link connecting; sleep 3 || break; done"#;
    LOOP.replace("@RETRIES@", RETRIES)
        .replacen("@HOST@", &posix_quote(alias), 1)
}

fn windows_startup(alias: &str) -> String {
    const LOOP: &str = r#"$sikemuxSshRetries = 0; while ($true) { & ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 @HOST@; $sikemuxSshStatus = $LASTEXITCODE; [Console]::Write(([char]27 + '[0m' + [char]27 + '[?25h' + [char]27 + '[?1l' + [char]27 + '>' + [char]27 + '[?2004l' + [char]27 + '[?1000l' + [char]27 + '[?1002l' + [char]27 + '[?1003l' + [char]27 + '[?1006l')); if ($sikemuxSshStatus -eq 0) { break }; if ($sikemuxSshStatus -eq 130) { Write-Host 'SSH reconnect cancelled. Back at your local shell.'; break }; if ($sikemuxSshRetries -ge @RETRIES@) { Write-Host 'SSH could not reconnect after @RETRIES@ retries. Back at your local shell.'; break }; $sikemuxSshRetries++; Write-Host "SSH connection lost. Retrying ($sikemuxSshRetries/@RETRIES@) in 3 seconds... Press Ctrl-C to stop."; Start-Sleep -Seconds 3 }"#;
    let host = format!("'{}'", alias.replace('\'', "''"));
    LOOP.replace("@RETRIES@", RETRIES)
        .replacen("@HOST@", &host, 1)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn runs_ssh_with_the_alias_as_one_argument_from_any_login_shell() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("args");
        let ssh = directory.path().join("ssh");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            record.display()
        );
        std::fs::write(&ssh, script).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            directory.path().display(),
            std::env::var("PATH").unwrap()
        );
        let alias = "odd 'host' \\ $HOME";
        let shells: Vec<&str> = ["/bin/sh", "/bin/bash", "/bin/zsh"]
            .into_iter()
            .filter(|shell| std::path::Path::new(shell).exists())
            .collect();
        assert!(shells.contains(&"/bin/sh"), "no /bin/sh to run the loop");
        for shell in shells {
            sikemux_process::user_environment::command(shell)
                .arg("-c")
                .arg(super::startup(alias))
                .env("PATH", &path)
                .output()
                .unwrap();
            let args = std::fs::read_to_string(&record).unwrap();
            assert_eq!(args.lines().last(), Some(alias), "{shell}");
        }
    }
}
