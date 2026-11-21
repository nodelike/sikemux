//! Private copies of an agent's home, for Sikemux to change its config without
//! touching the person's own.

use std::path::{Path, PathBuf};

use crate::agent_catalog::{AgentKind, Home, PrivateHome};

/// The home `kind` keeps, when Sikemux gives its launches private copies of it.
pub fn private(kind: AgentKind) -> Option<(&'static Home, &'static PrivateHome)> {
    let home = kind.entry().home.as_ref()?;
    Some((home, home.private.as_ref()?))
}

/// Makes `destination` a private copy of `kind`'s home: of `source`, else of
/// the one the person's shell names, sharing the store its sessions are in.
pub fn private_home(
    kind: AgentKind,
    destination: &Path,
    source: Option<PathBuf>,
) -> Result<(), String> {
    let (_, private) =
        private(kind).ok_or_else(|| format!("{} keeps no private home", kind.as_str()))?;
    let source = source
        .or_else(|| kind.home_root())
        .ok_or("the home folder is unavailable")?;
    share_session_store(
        &source,
        private.session_store,
        private.session_store_is_folder,
    )
    .and_then(|()| copy_home(destination, &source, private.config, private.empty_config))
    .map_err(|error| error.to_string())
}

/// The variable naming the file `kind` keeps its sign-in in, inside the
/// person's own `home`, so a sign-in made in any private copy reaches them all.
pub fn shared_login(kind: AgentKind, home: &Path) -> Option<(String, String)> {
    let login = private(kind)?.1.login.as_ref()?;
    Some((
        login.variable.to_owned(),
        home.join(login.file).to_string_lossy().into_owned(),
    ))
}

/// Makes `home` a copy of `source`: every entry is linked back to the real
/// one, except the config file, which is copied afresh so the person's latest
/// settings come along.
fn copy_home(
    home: &Path,
    source: &Path,
    config_name: &str,
    empty_config: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    match std::fs::read_dir(source) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.file_name() == config_name {
                    continue;
                }
                link_entry(&entry.path(), &home.join(entry.file_name()))?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let config = home.join(config_name);
    let source_config = source.join(config_name);
    if source_config.is_file() {
        std::fs::copy(source_config, &config)?;
    } else if !config.is_file() {
        std::fs::write(&config, empty_config)?;
    }
    crate::os::make_private_folder(home)?;
    crate::os::make_private_file(&config)
}

/// Makes the store an agent keeps its sessions in inside its real home, if it
/// has none yet, so a private home links to it rather than starting its own.
/// Sikemux lists and resumes sessions from the real home.
fn share_session_store(real_home: &Path, name: &str, folder: bool) -> std::io::Result<()> {
    let store = real_home.join(name);
    if std::fs::symlink_metadata(&store).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(real_home)?;
    if folder {
        std::fs::create_dir_all(&store)
    } else {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&store)
            .map(|_| ())
    }
}

fn link_entry(source: &Path, destination: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(destination).is_ok() {
        return Ok(());
    }
    crate::os::symlink(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_store_is_made_in_the_real_home_only_when_missing() {
        let home = tempfile::tempdir().unwrap();
        share_session_store(home.path(), "sessions", true).unwrap();
        share_session_store(home.path(), "state.db", false).unwrap();
        assert!(home.path().join("sessions").is_dir());
        assert_eq!(
            std::fs::metadata(home.path().join("state.db"))
                .unwrap()
                .len(),
            0
        );

        std::fs::write(home.path().join("state.db"), "kept").unwrap();
        share_session_store(home.path(), "state.db", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("state.db")).unwrap(),
            "kept"
        );
    }

    #[test]
    fn a_grok_sign_in_in_a_terminal_reaches_its_chat() {
        let data = tempfile::tempdir().unwrap();
        let own = data.path().join("own");
        let terminal = data.path().join("browser/grok/agent-one");
        let chat = data.path().join("browser/grok-chat/agent-one");
        for home in [&terminal, &chat] {
            private_home(AgentKind::Grok, home, Some(own.clone())).unwrap();
        }
        let (variable, login) = shared_login(AgentKind::Grok, &own).unwrap();
        let saved_in = |home: &Path, env: &[(String, String)]| {
            env.iter()
                .find(|(name, _)| *name == variable)
                .map_or_else(|| home.join("auth.json"), |(_, path)| PathBuf::from(path))
        };
        let env = [(variable.clone(), login)];
        std::fs::write(saved_in(&terminal, &env), "signed in").unwrap();
        assert_eq!(
            std::fs::read_to_string(saved_in(&chat, &env)).unwrap(),
            "signed in"
        );
        assert!(!saved_in(&chat, &[]).exists());
    }
}
