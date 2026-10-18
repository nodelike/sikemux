use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Variables every child process gets on top of this process's own
/// environment. Launched from the Dock, the app has none of the user's shell
/// setup, so the app fills this in from their login shell instead of writing
/// into its own environment, which is unsafe once other threads are running.
#[derive(Default)]
pub struct UserEnvironment {
    pub variables: HashMap<String, String>,
    /// Everything the person's login shell sets, children given it or not.
    pub shell: HashMap<String, String>,
}

struct Source {
    build: fn() -> UserEnvironment,
    /// Changes when what `build` reads from has changed, such as a login shell
    /// that answered after the first build gave up waiting for it.
    generation: fn() -> u64,
}

static SOURCE: OnceLock<Source> = OnceLock::new();
static ENVIRONMENT: RwLock<Option<(u64, Arc<UserEnvironment>)>> = RwLock::new(None);

/// Registers how to build the environment, and how to tell it has gone stale.
/// A process that never registers one gives its children exactly what it
/// inherited.
pub fn provide(build: fn() -> UserEnvironment, generation: fn() -> u64) {
    let _ = SOURCE.set(Source { build, generation });
}

/// Builds the environment now so the first spawn does not pay for it.
pub fn warm() {
    let _ = environment();
}

fn environment() -> Arc<UserEnvironment> {
    let Some(source) = SOURCE.get() else {
        return Arc::default();
    };
    let generation = (source.generation)();
    if let Ok(cached) = ENVIRONMENT.read() {
        if let Some((built, environment)) = cached.as_ref() {
            if *built == generation {
                return environment.clone();
            }
        }
    }
    let mut cached = ENVIRONMENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((built, environment)) = cached.as_ref() {
        if *built == generation {
            return environment.clone();
        }
    }
    let environment = Arc::new((source.build)());
    *cached = Some((generation, environment.clone()));
    environment
}

/// A `Command` that runs with the user's environment, and finds `program` on
/// the user's `PATH`.
#[allow(clippy::disallowed_methods)]
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let program = program.as_ref();
    let mut command = match git_program().filter(|_| program == "git") {
        Some(git) => Command::new(git),
        None => Command::new(program),
    };
    command.envs(&environment().variables);
    command
}

/// The git a bare `git` runs on the user's `PATH`, found once per `PATH`.
pub fn git_program() -> Option<PathBuf> {
    static FOUND: Mutex<Option<(OsString, PathBuf)>> = Mutex::new(None);
    let path = var_os("PATH").unwrap_or_default();
    let mut found = FOUND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((searched, git)) = found.as_ref() {
        if *searched == path && git.is_file() {
            return Some(git.clone());
        }
    }
    let git = past_xcode_stub(find_executable("git")?);
    *found = Some((path, git.clone()));
    Some(git)
}

/// macOS's `/usr/bin/git` asks `xcrun` where the real git is on every run,
/// which takes longer than most of the reads the app makes.
#[cfg(target_os = "macos")]
#[allow(clippy::disallowed_methods)]
fn past_xcode_stub(git: PathBuf) -> PathBuf {
    if git != std::path::Path::new("/usr/bin/git") {
        return git;
    }
    let output = Command::new("/usr/bin/xcrun")
        .args(["--find", "git"])
        .envs(&environment().variables)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let real = output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));
    match real {
        Some(real) if real.is_absolute() && real.is_file() => real,
        _ => git,
    }
}

#[cfg(not(target_os = "macos"))]
fn past_xcode_stub(git: PathBuf) -> PathBuf {
    git
}

/// Reads a variable the way a child process would see it.
pub fn var(name: &str) -> Option<String> {
    environment()
        .variables
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
}

/// A variable as the person's own login shell sets it, else as this process
/// has it, whether or not children are given it.
pub fn shell_var(name: &str) -> Option<String> {
    environment()
        .shell
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
}

/// Like `var`, but for values that may not be valid UTF-8, such as paths.
pub fn var_os(name: &str) -> Option<std::ffi::OsString> {
    environment()
        .variables
        .get(name)
        .map(Into::into)
        .or_else(|| std::env::var_os(name))
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    find_executables(name).into_iter().next()
}

/// Every program called `name` on the user's `PATH`, in its order.
pub fn find_executables(name: &str) -> Vec<PathBuf> {
    let Some(paths) = var_os("PATH") else {
        return Vec::new();
    };
    #[cfg(windows)]
    let names: Vec<String> = if PathBuf::from(name).extension().is_some() {
        vec![name.to_string()]
    } else {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|extension| !extension.is_empty())
            .map(|extension| format!("{name}{}", extension.to_ascii_lowercase()))
            .chain(std::iter::once(name.to_string()))
            .collect()
    };
    #[cfg(not(windows))]
    let names = vec![name.to_string()];

    find_executables_in(std::env::split_paths(&paths), &names)
}

fn find_executables_in(paths: impl IntoIterator<Item = PathBuf>, names: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for directory in paths {
        for candidate_name in names {
            let candidate = directory.join(candidate_name);
            if !candidate.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let Ok(metadata) = candidate.metadata() else {
                    continue;
                };
                if metadata.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            if !found.contains(&candidate) {
                found.push(candidate);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::find_executables_in;

    #[test]
    fn executable_lookup_returns_every_healthy_candidate_in_path_order() {
        let first = tempfile::tempdir().expect("first path");
        let second = tempfile::tempdir().expect("second path");
        let first_candidate = first.path().join("tool");
        let second_candidate = second.path().join("tool");
        std::fs::write(&first_candidate, b"first").expect("first executable");
        std::fs::write(&second_candidate, b"second").expect("second executable");

        #[cfg(unix)]
        for candidate in [&first_candidate, &second_candidate] {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(candidate).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(candidate, permissions).unwrap();
        }

        let names = vec!["tool".to_string()];
        assert_eq!(
            find_executables_in(
                [first.path().to_path_buf(), second.path().to_path_buf()],
                &names,
            ),
            vec![first_candidate, second_candidate]
        );
    }
}
