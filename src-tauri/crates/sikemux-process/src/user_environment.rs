use std::collections::HashMap;
use std::ffi::OsStr;
use std::process::Command;
use std::sync::OnceLock;

/// Variables every child process gets on top of this process's own
/// environment. Launched from the Dock, the app has none of the user's shell
/// setup, so the app fills this in from their login shell instead of writing
/// into its own environment, which is unsafe once other threads are running.
#[derive(Default)]
pub struct UserEnvironment {
    pub variables: HashMap<String, String>,
}

static SOURCE: OnceLock<fn() -> UserEnvironment> = OnceLock::new();
static ENVIRONMENT: OnceLock<UserEnvironment> = OnceLock::new();

/// Registers how to build the environment. A process that never registers one
/// gives its children exactly what it inherited.
pub fn provide(source: fn() -> UserEnvironment) {
    let _ = SOURCE.set(source);
}

/// Builds the environment now so the first spawn does not pay for it. A
/// caller that arrives while this is still running waits for the same result.
pub fn warm() {
    let _ = environment();
}

fn environment() -> &'static UserEnvironment {
    ENVIRONMENT.get_or_init(|| SOURCE.get().map(|source| source()).unwrap_or_default())
}

/// A `Command` that runs with the user's environment, and finds `program` on
/// the user's `PATH`.
#[allow(clippy::disallowed_methods)]
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.envs(&environment().variables);
    command
}

/// Reads a variable the way a child process would see it.
pub fn var(name: &str) -> Option<String> {
    environment()
        .variables
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
}

/// Like `var`, but for values that may not be valid UTF-8, such as paths.
pub fn var_os(name: &str) -> Option<std::ffi::OsString> {
    environment()
        .variables
        .get(name)
        .map(Into::into)
        .or_else(|| std::env::var_os(name))
}
