include!("src/generated_command_names.rs");

use sha2::{Digest, Sha256};

fn main() {
    record_helper("sikemux-voice", "VOICE");
    record_helper("sikemux-sim", "SIM");
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
