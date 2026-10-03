include!("src/generated_command_names.rs");

use sha2::{Digest, Sha256};

fn main() {
    record_helper("sikemux-voice", "VOICE");
    record_helper("sikemux-sim", "SIM");
    record_build_identity();
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(IPC_COMMANDS));
    tauri_build::try_build(attributes).expect("failed to prepare Sikemux native capabilities");
}

/// The voice and simulator helpers are published beside each release instead of
/// shipping in the app, which downloads them when they are first needed. The app
/// only accepts the exact helper built with it, so its size and hash are recorded here.
fn record_helper(name: &str, key: &str) {
    let binaries = std::path::Path::new("binaries");
    std::fs::create_dir_all(binaries).expect("could not create src-tauri/binaries");
    println!("cargo:rerun-if-changed={}", binaries.display());
    let target = std::env::var("TARGET").unwrap_or_default();
    let candidates = [
        format!("{name}-{target}"),
        format!("{name}-universal-apple-darwin"),
    ];
    for asset in candidates {
        let Ok(bytes) = std::fs::read(binaries.join(&asset)) else {
            continue;
        };
        println!("cargo:rustc-env=SIKEMUX_{key}_HELPER_ASSET={asset}");
        println!("cargo:rustc-env=SIKEMUX_{key}_HELPER_SIZE={}", bytes.len());
        println!(
            "cargo:rustc-env=SIKEMUX_{key}_HELPER_SHA256={}",
            hex::encode(Sha256::digest(&bytes))
        );
        return;
    }
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
    "crates/sikemux-core/src",
    "crates/sikemux-core/Cargo.toml",
    "crates/sikemux-pty/src",
    "crates/sikemux-pty/Cargo.toml",
    "src/bin/sikemux-editor/core_mode.rs",
    "Cargo.lock",
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
