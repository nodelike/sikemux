use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::async_runtime::spawn_blocking;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct JavaRuntime {
    pub home: String,
    pub major: u32,
}

#[derive(Serialize)]
pub struct JavaRuntimes {
    /// The Java a shell command gets today: `JAVA_HOME`, or macOS's default.
    pub default: Option<JavaRuntime>,
    pub installed: Vec<JavaRuntime>,
}

pub fn java_major(home: &Path) -> Option<u32> {
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let version = release
        .lines()
        .find_map(|line| line.strip_prefix("JAVA_VERSION="))?
        .trim_matches('"');
    let version = version.strip_prefix("1.").unwrap_or(version);
    version
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Homebrew's `opt/openjdk@N` folders keep the JDK two levels down.
fn jdk_home(path: &Path) -> Option<PathBuf> {
    [
        path.to_path_buf(),
        path.join("Contents/Home"),
        path.join("libexec/openjdk.jdk/Contents/Home"),
    ]
    .into_iter()
    .find(|home| home.join("release").is_file() && home.join("bin/java").is_file())
}

fn runtime_at(path: &Path) -> Option<JavaRuntime> {
    let home = jdk_home(path)?;
    let major = java_major(&home)?;
    Some(JavaRuntime {
        home: home.to_string_lossy().into_owned(),
        major,
    })
}

fn children(dir: PathBuf) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn candidate_folders() -> Vec<PathBuf> {
    let home = sikemux_process::user_environment::var_os("HOME").map(PathBuf::from);
    let mut folders = Vec::new();
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        folders.extend(children(PathBuf::from(prefix)).into_iter().filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("openjdk"))
        }));
    }
    folders.extend(children(PathBuf::from("/Library/Java/JavaVirtualMachines")));
    if let Some(home) = home {
        for dir in [
            "Library/Java/JavaVirtualMachines",
            ".sdkman/candidates/java",
            ".local/share/mise/installs/java",
            ".asdf/installs/java",
            ".gradle/jdks",
            ".jdks",
        ] {
            folders.extend(children(home.join(dir)));
        }
    }
    folders
}

pub fn installed_java() -> Vec<JavaRuntime> {
    let mut runtimes: Vec<JavaRuntime> = Vec::new();
    for folder in candidate_folders() {
        let Some(runtime) = runtime_at(&folder) else {
            continue;
        };
        let canonical = std::fs::canonicalize(&runtime.home).ok();
        let seen = runtimes
            .iter()
            .any(|known| std::fs::canonicalize(&known.home).ok() == canonical);
        if !seen {
            runtimes.push(runtime);
        }
    }
    runtimes.sort_by_key(|runtime| runtime.major);
    runtimes
}

pub fn default_java() -> Option<JavaRuntime> {
    if let Some(home) = sikemux_process::user_environment::var_os("JAVA_HOME") {
        return runtime_at(Path::new(&home));
    }
    let output = sikemux_process::user_environment::command("/usr/libexec/java_home")
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    runtime_at(Path::new(String::from_utf8_lossy(&output.stdout).trim()))
}

/// The installed Java closest to `major` without going under it.
pub fn java_at_least(major: u32) -> Option<JavaRuntime> {
    installed_java()
        .into_iter()
        .filter(|runtime| runtime.major >= major)
        .min_by_key(|runtime| runtime.major)
}

#[tauri::command]
pub async fn java_runtimes() -> AppResult<JavaRuntimes> {
    spawn_blocking(|| JavaRuntimes {
        default: default_java(),
        installed: installed_java(),
    })
    .await
    .map_err(|e| AppError::Other(format!("join: {e}")))
}

#[tauri::command]
pub async fn install_java(major: u32) -> AppResult<JavaRuntime> {
    if !(8..=40).contains(&major) {
        return Err(AppError::BadArg("unsupported Java version"));
    }
    spawn_blocking(move || {
        let formula = format!("openjdk@{major}");
        let output = sikemux_process::user_environment::command("brew")
            .args(["install", &formula])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    AppError::Other(
                        "Homebrew not found on PATH. Install Homebrew first: https://brew.sh"
                            .into(),
                    )
                } else {
                    AppError::Other(format!("run brew install {formula}: {e}"))
                }
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Other(format!(
                "brew install {formula} failed: {}",
                stderr.trim()
            )));
        }
        installed_java()
            .into_iter()
            .find(|runtime| runtime.major == major)
            .ok_or_else(|| {
                AppError::Other(format!(
                    "{formula} installed, but Sikemux could not find Java {major}"
                ))
            })
    })
    .await
    .map_err(|e| AppError::Other(format!("join: {e}")))?
}
