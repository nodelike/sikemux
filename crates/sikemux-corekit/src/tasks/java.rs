use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct JavaRuntime {
    pub home: String,
    pub major: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct JavaRuntimes {
    /// The Java a shell command gets today: `JAVA_HOME`, else the system's default.
    pub default: Option<JavaRuntime>,
    pub installed: Vec<JavaRuntime>,
}

/// The Java a task with a required version runs on.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaChoice {
    pub required: u32,
    /// None when nothing installed is new enough.
    pub runtime: Option<JavaRuntime>,
    /// The Java a plain shell would have used.
    pub current: Option<JavaRuntime>,
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

fn children(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn candidate_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    for prefix in [
        "/opt/homebrew/opt",
        "/usr/local/opt",
        "/home/linuxbrew/.linuxbrew/opt",
    ] {
        folders.extend(children(prefix).into_iter().filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("openjdk"))
        }));
    }
    for dir in [
        "/Library/Java/JavaVirtualMachines",
        "/usr/lib/jvm",
        "/usr/java",
    ] {
        folders.extend(children(dir));
    }
    if let Some(home) = sikemux_process::user_environment::var_os("HOME").map(PathBuf::from) {
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
    if cfg!(target_os = "macos") {
        let output = sikemux_process::user_environment::command("/usr/libexec/java_home")
            .output()
            .ok()
            .filter(|output| output.status.success())?;
        return runtime_at(Path::new(String::from_utf8_lossy(&output.stdout).trim()));
    }
    let java =
        std::fs::canonicalize(sikemux_process::user_environment::find_executable("java")?).ok()?;
    runtime_at(java.parent()?.parent()?)
}

pub fn runtimes() -> JavaRuntimes {
    JavaRuntimes {
        default: default_java(),
        installed: installed_java(),
    }
}

/// The installed Java closest to `major` without going under it.
pub fn java_at_least(major: u32) -> Option<JavaRuntime> {
    installed_java()
        .into_iter()
        .filter(|runtime| runtime.major >= major)
        .min_by_key(|runtime| runtime.major)
}

/// The exact version asked for, else the shell's own Java if it is new
/// enough, else the closest newer one.
pub fn choose(required: u32, runtimes: &JavaRuntimes) -> JavaChoice {
    let current = runtimes.default.clone();
    let exact = match &current {
        Some(current) if current.major == required => Some(current.clone()),
        _ => runtimes
            .installed
            .iter()
            .find(|runtime| runtime.major == required)
            .cloned(),
    };
    let newer = runtimes
        .installed
        .iter()
        .filter(|runtime| runtime.major > required)
        .min_by_key(|runtime| runtime.major)
        .cloned();
    let runtime = exact.or_else(|| match &current {
        Some(current) if current.major > required => Some(current.clone()),
        _ => newer,
    });
    JavaChoice {
        required,
        runtime,
        current,
    }
}

pub fn install_java(major: u32) -> Result<JavaRuntime, String> {
    if !(8..=40).contains(&major) {
        return Err("unsupported Java version".into());
    }
    let formula = format!("openjdk@{major}");
    super::tools::brew_install(&formula)?;
    installed_java()
        .into_iter()
        .find(|runtime| runtime.major == major)
        .ok_or_else(|| format!("{formula} installed, but Sikemux could not find Java {major}"))
}
