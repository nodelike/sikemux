use std::{env, fs, path::PathBuf};

/// The core ships inside the app and takes its release version from the app's manifest.
fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = fs::read_to_string(&manifest).expect("the app's Cargo.toml is readable");
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("the app's Cargo.toml names a version");
    println!("cargo:rustc-env=SIKEMUX_VERSION={version}");
}
