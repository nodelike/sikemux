//! The zsh agents run their commands in: the person's own, read from their
//! own startup files, with three options on top. Without them zsh stops a
//! whole command line at a glob that matches nothing, reads `echo ===` as a
//! command lookup, and keeps `$FILES` whole, which agents written for bash
//! tripped over hundreds of times. The person's own terminals are untouched.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sikemux_process::vars;

const OPTIONS: &str = "setopt nonomatch noequals shwordsplit";
/// Each startup file zsh reads from `ZDOTDIR`, and whether the options go
/// after the person's copy of it: in `.zshrc` for an interactive shell, which
/// reads it last, and in `.zshenv` for one that is not, which never reads it.
const FILES: [(&str, Option<&str>); 4] = [
    (".zshenv", Some("[[ -o interactive ]] || ")),
    (".zprofile", None),
    (".zshrc", Some("")),
    (".zlogin", None),
];

static FOLDER: OnceLock<PathBuf> = OnceLock::new();

/// Writes the startup files into a folder of `data_dir`, for agents started
/// from now on.
pub(crate) fn install(data_dir: &Path) {
    let folder = data_dir.join("agent-zsh");
    let written = FILES.iter().try_for_each(|(name, options)| {
        sikemux_process::files::write_atomic(
            &folder.join(name),
            startup(name, *options).as_bytes(),
            false,
        )
    });
    match written {
        Ok(()) => {
            let _ = FOLDER.set(folder);
        }
        Err(error) => eprintln!("sikemux core: agents' zsh keeps its stock options: {error}"),
    }
}

/// Reads the person's own `name`, from where their zsh reads it, which their
/// `.zshenv` may move, then comes back here for the next file.
fn startup(name: &str, options: Option<&str>) -> String {
    let user = format!("${{{}:-$HOME}}", vars::USER_ZDOTDIR);
    let mut text = format!(
        "_sikemux_zdotdir=$ZDOTDIR\nZDOTDIR={user}\n[[ -f $ZDOTDIR/{name} ]] && source $ZDOTDIR/{name}\nexport {}=$ZDOTDIR\nZDOTDIR=$_sikemux_zdotdir\nunset _sikemux_zdotdir\n",
        vars::USER_ZDOTDIR
    );
    if let Some(guard) = options {
        text.push_str(&format!("{guard}{OPTIONS}\n"));
    }
    text
}

/// Gives a process started from an agent's zsh the person's own `ZDOTDIR`
/// back. Kept, it would make every agent's zsh read these files as the
/// person's own, and source them over and over. Safe only before other
/// threads run.
pub(crate) fn leave() {
    let Some(user) = std::env::var_os(vars::USER_ZDOTDIR) else {
        return;
    };
    if std::env::var_os("HOME").is_some_and(|home| home == user) {
        std::env::remove_var("ZDOTDIR");
    } else {
        std::env::set_var("ZDOTDIR", user);
    }
    std::env::remove_var(vars::USER_ZDOTDIR);
}

/// What an agent runs with for its zsh to read these files. Nothing when the
/// person's shell is not zsh, or before [`install`].
pub(crate) fn environment() -> Vec<(String, String)> {
    let Some(folder) = FOLDER.get() else {
        return Vec::new();
    };
    let user_shell = sikemux_process::user_environment::var("SHELL").unwrap_or_default();
    if !user_shell.ends_with("/zsh") {
        return Vec::new();
    }
    let user_zdotdir = sikemux_process::user_environment::var("ZDOTDIR")
        .or_else(|| sikemux_process::user_environment::var("HOME"))
        .unwrap_or_default();
    vec![
        ("ZDOTDIR".to_owned(), folder.to_string_lossy().into_owned()),
        (vars::USER_ZDOTDIR.to_owned(), user_zdotdir),
    ]
}
