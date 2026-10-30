use sha2::{Digest, Sha256};

fn main() {
    record_build_identity();
}

// The build runs in the developer's own shell, which already has their PATH.
#[allow(clippy::disallowed_methods)]
fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

/// Everything the background core is compiled from. A change to any of it
/// makes a new build that a running core upgrades to.
const CORE_SOURCES: &[&str] = &[
    "../sikemux-core/src",
    "../sikemux-core/Cargo.toml",
    "../sikemux-pty/src",
    "../sikemux-pty/Cargo.toml",
    "../sikemux-client/src",
    "../sikemux-client/Cargo.toml",
    "../sikemux-wire/src",
    "../sikemux-wire/Cargo.toml",
    "src/core_mode.rs",
    "../../Cargo.lock",
];

fn collect_files(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_files(&entry.path(), files);
        }
    } else if path.is_file() {
        files.push(path.to_path_buf());
    }
}

/// The app and its sidecar are compiled separately, at different times and
/// with different features, so they compare this instead of build times.
fn core_source_fingerprint() -> String {
    let mut files = Vec::new();
    for source in CORE_SOURCES {
        println!("cargo:rerun-if-changed={source}");
        collect_files(std::path::Path::new(source), &mut files);
    }
    files.sort();
    let mut digest = Sha256::new();
    for file in files {
        digest.update(file.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(std::fs::read(&file).unwrap_or_default());
        digest.update([0]);
    }
    hex::encode(digest.finalize()).chars().take(16).collect()
}

/// The commit, time and core sources this binary was built from, which the
/// background core reports so an app can tell which build it is talking to.
fn record_build_identity() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let built_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=SIKEMUX_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=SIKEMUX_BUILD_TIME={built_at}");
    println!(
        "cargo:rustc-env=SIKEMUX_BUILD_SOURCE={}",
        core_source_fingerprint()
    );
    let head_ref = git(&["symbolic-ref", "-q", "HEAD"]);
    let watched = ["HEAD", "packed-refs"]
        .into_iter()
        .map(str::to_string)
        .chain(head_ref);
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name]) {
            if std::path::Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}
