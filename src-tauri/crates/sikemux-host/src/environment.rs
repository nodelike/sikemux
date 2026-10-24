//! The environment every process the app or the core starts gets: the PATH,
//! locale and SSH agent of the person's login shell.

use std::collections::HashMap;
use std::path::Path;

use sikemux_process::user_environment::UserEnvironment;
use sikemux_pty::user_shell::login_shell_environment;
#[cfg(unix)]
use sikemux_pty::user_shell::login_shell_path;

/// macOS GUI apps launched from Finder/Dock inherit launchd's minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`), missing `~/.local/bin`,
/// `/opt/homebrew/bin`, `~/.cargo/bin`, `~/.opencode/bin`, etc. that the
/// user actually has tools in. `make dev` works because the dev binary is
/// launched from a terminal that already has the right PATH.
///
/// So every child gets the `PATH` the user's login shell resolves, its
/// locale and the SSH agent it points at. Reading the shell takes as long as
/// the user's rc files do, so it runs beside window creation rather than ahead
/// of it.
pub fn user_environment() -> UserEnvironment {
    let shell = login_shell_environment();
    let mut variables = HashMap::new();
    // An app opened from the Dock has no locale, so tools fall back to ASCII.
    for (key, value) in sikemux_pty::user_shell::login_shell_locale() {
        variables.insert(key.clone(), value.clone());
    }
    // 1Password, Secretive and gpg-agent are set up by exporting the socket in
    // the shell profile, which launchd's own agent would otherwise shadow.
    if let Some(socket) = shell
        .get("SSH_AUTH_SOCK")
        .filter(|socket| !socket.is_empty())
    {
        variables.insert("SSH_AUTH_SOCK".to_string(), socket.clone());
    }
    if let Some(path) = user_path() {
        variables.insert("PATH".to_string(), path);
    }
    UserEnvironment {
        variables,
        shell: shell.clone(),
    }
}

#[cfg(unix)]
fn user_path() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let inherited = std::env::var("PATH").unwrap_or_default();
    if sikemux_process::user_environment::search_path_only() {
        return Some(inherited);
    }
    let cli = crate::cli_paths::cli_link_directory().and_then(Path::to_str);
    Some(join_user_path(
        cli,
        login_shell_path().unwrap_or_default(),
        &home,
        &inherited,
    ))
}

/// The login shell's PATH when it could be read, then the common user-local
/// folders, which can live in rc files a login shell does not source, then
/// whatever the app was started with and the system folders. Reading the
/// shell can time out, so the last two are what keep children working then.
#[cfg(unix)]
fn join_user_path(cli: Option<&str>, shell_path: &str, home: &str, inherited: &str) -> String {
    let extra = [
        format!("{home}/.local/bin"),
        format!("{home}/.cargo/bin"),
        format!("{home}/.opencode/bin"),
        format!("{home}/.config/shell/bin"),
        format!("{home}/go/bin"),
        // Node tooling: typescript-language-server, pyright, vue-lsp, etc.
        // installed via `pnpm add -g` land here on macOS.
        format!("{home}/Library/pnpm"),
        format!("{home}/.npm/bin"),
        format!("{home}/.bun/bin"),
        // Python virtualenv tooling (pipx, pyenv shims).
        format!("{home}/.pyenv/shims"),
        // Node version managers, where node, npm and the agent CLIs usually
        // live; the shell's PATH has them only when it could be read.
        format!("{home}/.volta/bin"),
        format!("{home}/.local/share/mise/shims"),
        format!("{home}/.asdf/shims"),
        format!("{home}/Library/Application Support/fnm/aliases/default/bin"),
        format!("{home}/.local/share/fnm/aliases/default/bin"),
        format!("{home}/.claude/local"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        "/opt/local/bin".to_string(),
    ];
    let nvm = newest_nvm_bin(home);
    let mut parts: Vec<&str> = cli.into_iter().collect();
    let candidates = shell_path
        .split(':')
        .chain(extra.iter().map(String::as_str))
        .chain(nvm.as_deref())
        .chain(inherited.split(':'))
        .chain(sikemux_process::paths::SYSTEM_PATH);
    for part in candidates.filter(|part| !part.is_empty()) {
        if !parts.contains(&part) {
            parts.push(part);
        }
    }
    parts.join(":")
}

/// Windows desktop applications inherit the user's PATH, so only the CLI link
/// needs adding.
#[cfg(windows)]
fn user_path() -> Option<String> {
    let directory = crate::cli_paths::cli_link_directory()?;
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(directory.to_path_buf())
        .chain(std::env::split_paths(&existing).filter(|path| path != directory));
    std::env::join_paths(paths).ok()?.into_string().ok()
}

/// nvm keeps each Node it installed in its own folder and puts one on PATH
/// only in a shell that loaded it, so the newest stands in for it.
#[cfg(unix)]
fn newest_nvm_bin(home: &str) -> Option<String> {
    let versions = std::fs::read_dir(Path::new(home).join(".nvm/versions/node")).ok()?;
    let parse = |name: &str| -> Option<(u64, u64, u64)> {
        let mut parts = name
            .strip_prefix('v')?
            .split('.')
            .map(|part| part.parse().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    versions
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((parse(&name)?, entry.path().join("bin")))
        })
        .filter(|(_, bin)| bin.is_dir())
        .max_by_key(|(version, _)| *version)
        .map(|(_, bin)| bin.to_string_lossy().into_owned())
}

/// The PATH this app worked out for the person's tools, for a process the
/// background core starts. The core keeps the environment it first started
/// with through every update, so the app names the PATH on each launch.
pub fn child_path() -> Option<String> {
    sikemux_process::user_environment::var_os("PATH")
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_system_folders_stay_on_path_when_the_login_shell_cannot_be_read() {
        let path = join_user_path(Some("/Users/me/.config/sikemux/bin"), "", "/Users/me", "");
        let parts: Vec<&str> = path.split(':').collect();
        assert_eq!(parts.first(), Some(&"/Users/me/.config/sikemux/bin"));
        for system in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            assert!(parts.contains(&system), "{system} missing from {path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_path_leads_and_nothing_repeats() {
        let path = join_user_path(
            None,
            "/Users/me/.nvm/bin:/usr/bin:/bin",
            "/Users/me",
            "/usr/bin:/bin:/usr/sbin:/sbin",
        );
        let parts: Vec<&str> = path.split(':').collect();
        assert_eq!(&parts[..3], ["/Users/me/.nvm/bin", "/usr/bin", "/bin"]);
        let mut seen = std::collections::HashSet::new();
        assert!(parts.iter().all(|part| seen.insert(*part)), "{path}");
    }
}
