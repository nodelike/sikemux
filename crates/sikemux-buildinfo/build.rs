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

/// What the core embeds from outside the `src` of the crates it links, from the workspace root.
const EMBEDDED: &[&str] = &[
    "crates/sikemux-agents/resources",
    "crates/sikemux-agents/models-dev.json",
    "crates/sikemux-agents/model-providers.json",
    "tools",
    "app/Cargo.toml",
    "Cargo.lock",
];

/// The `path` of every dependency `manifest` writes as `name = { path = "..." }`.
fn path_dependencies(manifest: &str) -> Vec<&str> {
    manifest
        .lines()
        .filter_map(|line| {
            let (_, table) = line.split_once("= {")?;
            let (_, path) = table.split_once("path = \"")?;
            path.split('"').next()
        })
        .collect()
}

/// Everything the background core is compiled from, found by following the path
/// dependencies from sikemux-host. A change to any of it makes a new build that a
/// running core upgrades to.
fn core_sources(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut crates = std::collections::BTreeSet::new();
    let mut waiting = vec![std::path::PathBuf::from("../sikemux-host")];
    while let Some(dir) = waiting.pop() {
        let Ok(dir) = std::fs::canonicalize(dir) else {
            continue;
        };
        if crates.contains(&dir) {
            continue;
        }
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        waiting.extend(
            path_dependencies(&manifest)
                .into_iter()
                .map(|path| dir.join(path)),
        );
        crates.insert(dir);
    }
    crates
        .iter()
        .flat_map(|dir| [dir.join("src"), dir.join("Cargo.toml")])
        .chain(EMBEDDED.iter().map(|path| root.join(path)))
        .collect()
}

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

/// The app and its core are compiled separately, at different times and
/// with different features, so they compare this instead of build times.
fn core_source_fingerprint() -> String {
    let root = std::fs::canonicalize("../..").unwrap_or_default();
    let mut files = Vec::new();
    for source in core_sources(&root) {
        let inside = source.strip_prefix(&root).unwrap_or(&source);
        println!("cargo:rerun-if-changed=../../{}", inside.display());
        collect_files(&source, &mut files);
    }
    files.sort();
    let mut digest = Sha256::new();
    for file in files {
        let name = file.strip_prefix(&root).unwrap_or(&file);
        digest.update(name.to_string_lossy().as_bytes());
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
