#[cfg(unix)]
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn executable_in_path(bin: &str) -> Option<PathBuf> {
    sikemux_process::user_environment::find_executable(bin)
}

fn go_env(name: &str) -> Option<String> {
    let output = sikemux_process::user_environment::command("go")
        .args(["env", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn go_path_list(name: &str) -> Vec<PathBuf> {
    go_env(name)
        .map(|value| std::env::split_paths(&std::ffi::OsString::from(value)).collect())
        .unwrap_or_default()
}

fn go_lsp_path() -> Option<PathBuf> {
    executable_in_path("gopls")
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .and_then(|home| path_if_executable(home.join("go").join("bin").join("gopls")))
        })
        .or_else(|| {
            go_path_list("GOBIN")
                .into_iter()
                .find_map(|p| path_if_executable(p.join("gopls")))
        })
        .or_else(|| {
            go_path_list("GOPATH")
                .into_iter()
                .find_map(|p| path_if_executable(p.join("bin").join("gopls")))
        })
}

fn path_if_executable(path: PathBuf) -> Option<PathBuf> {
    is_executable(&path).then_some(path)
}

fn path_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

// (bin, args) tuple for a language. Order matters only for display.
//
// Built-in table covers the common cases; users override or extend per-
// language via an env var `SIKEMUX_LSP_<UPPER_LANG>="<bin> <arg>..."`.
// Example: `SIKEMUX_LSP_RUST="rust-analyzer --log /tmp/ra.log"`. The
// override applies to any matching language, including ones we didn't
// ship with — e.g. `SIKEMUX_LSP_LUA="lua-language-server"`.
const BUILTIN_LSP: &[(&str, &str, &[&str])] = &[
    ("typescript", "typescript-language-server", &["--stdio"]),
    ("javascript", "typescript-language-server", &["--stdio"]),
    ("go", "gopls", &[]),
    ("rust", "rust-analyzer", &[]),
    ("python", "pyright-langserver", &["--stdio"]),
    ("java", "jdtls", &[]),
];

const OVERRIDE_PREFIX: &str = "SIKEMUX_LSP_";

/// Every language a server can start for: the built-in ones and any that a
/// `SIKEMUX_LSP_<LANG>` variable names.
pub(super) fn server_languages() -> Vec<String> {
    let shell = sikemux_pty::user_shell::login_shell_environment()
        .keys()
        .cloned();
    let mut languages: Vec<String> = BUILTIN_LSP
        .iter()
        .map(|(language, _, _)| (*language).to_owned())
        .chain(
            shell
                .chain(std::env::vars().map(|(key, _)| key))
                .filter_map(|key| Some(key.strip_prefix(OVERRIDE_PREFIX)?.to_lowercase()))
                .filter(|language| server_command(language).is_some()),
        )
        .collect();
    languages.sort();
    languages.dedup();
    languages
}

const JDTLS_MIN_JAVA: u32 = 21;

/// jdtls runs on the Java in `JAVA_HOME`, so one set for an older project would stop it starting.
fn jdtls_java_home() -> Option<PathBuf> {
    let current = sikemux_core::tasks::java::default_java()?;
    if current.major >= JDTLS_MIN_JAVA {
        return None;
    }
    sikemux_core::tasks::java::java_at_least(JDTLS_MIN_JAVA)
        .map(|runtime| PathBuf::from(runtime.home))
}

pub(super) fn server_env(language: &str) -> Vec<(&'static str, PathBuf)> {
    match language {
        "java" => jdtls_java_home()
            .map(|home| vec![("JAVA_HOME", home)])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub(super) fn server_command(language: &str) -> Option<(String, Vec<String>)> {
    let env_key = format!("{OVERRIDE_PREFIX}{}", language.to_uppercase());
    if let Some(spec) = sikemux_process::user_environment::shell_var(&env_key) {
        // Naive split — sufficient for "bin arg1 arg2". Quoted args aren't
        // supported; users that need them can wrap in a shell script.
        let mut parts = spec.split_whitespace();
        let bin = parts.next()?.to_string();
        let args = parts.map(|s| s.to_string()).collect();
        return Some((bin, args));
    }
    for (lang, bin, args) in BUILTIN_LSP {
        if *lang == language {
            let resolved_bin = if language == "go" && *bin == "gopls" {
                go_lsp_path()
                    .map(path_string)
                    .unwrap_or_else(|| (*bin).to_string())
            } else {
                (*bin).to_string()
            };
            return Some((
                resolved_bin,
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
        }
    }
    None
}

fn install_output_message(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(stdout).trim().to_string();
    if stdout.is_empty() {
        "unknown failure".into()
    } else {
        stdout
    }
}

pub(super) fn install_gopls() -> AppResult<String> {
    let output = sikemux_process::user_environment::command("go")
        .args(["install", "golang.org/x/tools/gopls@latest"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::Lsp(
                    "Go toolchain not found on PATH. Install Go first: https://go.dev/doc/install"
                        .into(),
                )
            } else {
                AppError::Lsp(format!("run go install gopls: {e}"))
            }
        })?;
    if !output.status.success() {
        return Err(AppError::Lsp(format!(
            "go install golang.org/x/tools/gopls@latest failed: {}",
            install_output_message(&output.stdout, &output.stderr)
        )));
    }
    go_lsp_path().map(path_string).ok_or_else(|| {
        AppError::Lsp(
            "gopls installed, but Sikemux could not locate it in PATH, GOBIN, or GOPATH/bin".into(),
        )
    })
}

pub(super) fn install_jdtls() -> AppResult<String> {
    sikemux_core::tasks::tools::brew_install("jdtls").map_err(AppError::Lsp)?;
    executable_in_path("jdtls").map(path_string).ok_or_else(|| {
        AppError::Lsp("jdtls installed, but Sikemux could not locate it in PATH".into())
    })
}
