fn main() {
    // A program exports nothing on macOS. A thin-LTO link otherwise lists most of its Rust
    // functions in the export table, which strip leaves behind as megabytes of names.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-no_exported_symbols");
    }
    // Libraries the program's own libraries load come from Windows' folder alone, before main
    // can pin where later loads look.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
