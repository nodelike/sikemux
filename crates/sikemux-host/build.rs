/// A program exports nothing on macOS. A thin-LTO link otherwise lists most of its
/// Rust functions in the export table, which strip leaves behind as megabytes of names.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-no_exported_symbols");
    }
}
