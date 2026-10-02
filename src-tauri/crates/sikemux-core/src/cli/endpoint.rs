//! The file that tells the CLI and agents' tool server where Sikemux's tool
//! endpoint listens and which token it holds.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::protocol::CliEndpointDescriptor;

/// `~/.config/sikemux/cli.json`, `cli.dev.json` in debug builds, unless
/// `SIKEMUX_CLI_ENDPOINT_PUBLISH` names another file.
///
/// Deliberately not `SIKEMUX_CLI_ENDPOINT`: that one is handed to everything
/// Sikemux spawns so it can find the endpoint. Reading it here would let a
/// second Sikemux started from one of those terminals publish over it.
pub fn default_endpoint_path() -> Option<PathBuf> {
    if let Some(path) =
        std::env::var_os("SIKEMUX_CLI_ENDPOINT_PUBLISH").filter(|path| !path.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".config/sikemux")
            .join(if cfg!(debug_assertions) {
                "cli.dev.json"
            } else {
                "cli.json"
            }),
    )
}

/// Writes the file private to this user, through a rename so a reader never
/// sees half of it. A directory this call creates is private too; one that
/// exists keeps its permissions.
pub fn write_endpoint(path: &Path, descriptor: &CliEndpointDescriptor) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| std::io::Error::other("the CLI endpoint path has no directory"))?;
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut staging = path.as_os_str().to_owned();
    staging.push(format!(".{}.tmp", std::process::id()));
    let staging = PathBuf::from(staging);
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&staging)?;
    file.write_all(&serde_json::to_vec(descriptor)?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staging, path)
}

/// Removes the file only while it still names this token, so an endpoint
/// that another process published since is left alone.
pub fn remove_owned_endpoint(path: &Path, token: &str) {
    let owned = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CliEndpointDescriptor>(&bytes).ok())
        .is_some_and(|descriptor| descriptor.token == token);
    if owned {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(token: &str) -> CliEndpointDescriptor {
        CliEndpointDescriptor {
            protocol: 2,
            pid: 1,
            port: 42,
            token: token.into(),
            version: "test".into(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn endpoint_files_are_private_and_an_existing_directory_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/cli.json");
        write_endpoint(&path, &descriptor("token")).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        write_endpoint(&dir.path().join("cli.json"), &descriptor("token")).unwrap();
        assert_eq!(mode(dir.path()), 0o755);
    }

    #[test]
    fn only_the_publisher_removes_its_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli.json");
        write_endpoint(&path, &descriptor("ours")).unwrap();
        remove_owned_endpoint(&path, "theirs");
        assert!(path.exists());
        remove_owned_endpoint(&path, "ours");
        assert!(!path.exists());
    }
}
