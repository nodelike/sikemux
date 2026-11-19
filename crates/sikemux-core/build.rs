use std::{env, error::Error, fs, path::PathBuf};

/// The core ships inside the app and takes its release version from the app's manifest.
fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or("the app's Cargo.toml names no version")?;
    println!("cargo:rustc-env=SIKEMUX_VERSION={version}");
    Ok(())
}
