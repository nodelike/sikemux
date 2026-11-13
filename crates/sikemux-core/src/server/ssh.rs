//! The hosts in the person's SSH config, each with how its terminals are
//! reaching it, and a watch on the config that tells clients when it changes.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
use crate::server::ServerCore;
pub use sikemux_calls::ssh::{Reach, SshHost};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Weak;

use notify::{RecursiveMode, Watcher};
use sikemux_pty::shell_protocol::SshLink;

use super::changed::Topic;
use super::Core;

fn config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh/config"))
}

/// How deep `Include`s may nest, as ssh allows.
const INCLUDE_DEPTH: u8 = 16;

/// The hosts in the config and the files it includes, wildcard patterns left out since
/// nothing connects to them.
pub(crate) fn hosts(core: &Core) -> Vec<SshHost> {
    watch(core);
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
    if let Some(path) = config_path() {
        read_config(&path, None, 0, &reach, &mut Vec::new(), &mut hosts);
    }
    hosts
}

fn read_config(
    path: &Path,
    file: Option<String>,
    depth: u8,
    reach: &dyn Fn(&str) -> Reach,
    read: &mut Vec<PathBuf>,
    hosts: &mut Vec<SshHost>,
) {
    let whole = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if read.contains(&whole) {
        return;
    }
    read.push(whole);
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut aliases: Vec<String> = Vec::new();
    let (mut hostname, mut user, mut port, mut at) = (None, None, None, 0);
    let flush = |hosts: &mut Vec<SshHost>,
                 aliases: &[String],
                 hostname: &Option<String>,
                 user: &Option<String>,
                 port,
                 line| {
        for alias in aliases.iter().filter(|alias| !alias.contains(['*', '?'])) {
            if hosts.iter().any(|host| host.alias == *alias) {
                continue;
            }
            hosts.push(SshHost {
                alias: alias.clone(),
                hostname: hostname.clone(),
                user: user.clone(),
                port,
                line,
                file: file.clone(),
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
                flush(hosts, &aliases, &hostname, &user, port, at);
                aliases = value.split_whitespace().map(String::from).collect();
                (hostname, user, port, at) = (None, None, None, number);
            }
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()),
            "port" => port = value.parse().ok(),
            "include" if depth < INCLUDE_DEPTH => {
                for included in value.split_whitespace().flat_map(included_files) {
                    let shown = Some(included.to_string_lossy().into_owned());
                    read_config(&included, shown, depth + 1, reach, read, hosts);
                }
            }
            _ => {}
        }
    }
    flush(hosts, &aliases, &hostname, &user, port, at);
}

/// The files an `Include` names: from ~/.ssh when relative, with `~` for the home folder
/// and wildcards in the file's name.
fn included_files(pattern: &str) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if Path::new(pattern).is_absolute() => PathBuf::from(pattern),
        None => home.join(".ssh").join(pattern),
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let (Some(folder), Some(name)) = (path.parent(), name) else {
        return Vec::new();
    };
    if !name.contains(['*', '?', '[']) {
        return vec![path];
    }
    let Ok(matcher) = globset::Glob::new(&name).map(|glob| glob.compile_matcher()) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.file_name().is_some_and(|name| matcher.is_match(name))
        })
        .collect();
    found.sort();
    found
}

/// Starts watching the config once, the first time a client lists the hosts.
fn watch(core: &Core) {
    let Some(folder) = config_path().and_then(|path| path.parent().map(PathBuf::from)) else {
        return;
    };
    let weak: Weak<Core> = core.weak();
    core.ssh_watch().get_or_init(|| {
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let touches_config = event.is_ok_and(|event| {
                    event
                        .paths
                        .iter()
                        .any(|path| path.file_name().is_some_and(|name| name == "config"))
                });
                if let (true, Some(core)) = (touches_config, weak.upgrade()) {
                    core.changed(What::Read(Topic::Ssh));
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
    core.changed(What::Read(Topic::Ssh));
    core.changed(What::Read(Topic::Hosts));
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
                .arg(sikemux_serve::server::ssh_startup::startup(alias))
                .env("PATH", &path)
                .output()
                .unwrap();
            let args = std::fs::read_to_string(&record).unwrap();
            assert_eq!(args.lines().last(), Some(alias), "{shell}");
        }
    }
}
