//! The core on its own, without the rest of the `sikemux` sidecar, for tests
//! that need a core in a process of its own: `sikemux-core core …`.

#[cfg(unix)]
fn main() {
    if let Some(code) = sikemux_core::git_guard::shim::run_if_git() {
        std::process::exit(code);
    }
    let build = sikemux_core::protocol::BuildIdentity::new(
        env!("CARGO_PKG_VERSION"),
        "standalone",
        0,
        "standalone",
    );
    let args = std::env::args().skip(1).skip_while(|arg| arg == "core");
    std::process::exit(sikemux_core::server::main(args, build));
}

#[cfg(not(unix))]
fn main() {}
