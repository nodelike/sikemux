//! The `sikemux` command and `$EDITOR` inside Sikemux terminals. Agents start
//! the same binary with `--tools-mcp` to reach their browser and workspace tools,
//! and `sikemux core` runs the background process that owns terminals. Run as
//! `git`, it is the git agents find first on their PATH.

mod core_mode;
#[cfg(windows)]
mod service_mode;
mod tools_mcp;

use sikemux_host::cli_client;

fn main() {
    #[cfg(windows)]
    if let Err(error) = sikemux_trust::pin_library_search() {
        eprintln!("Sikemux could not limit where it loads libraries from: {error}");
        std::process::exit(1);
    }
    let first = std::env::args_os().nth(1);
    #[cfg(windows)]
    if first
        .as_ref()
        .is_some_and(|arg| arg == service_mode::SERVICE_ARG)
    {
        let action = std::env::args().nth(2);
        std::process::exit(service_mode::run(action.as_deref()));
    }
    let core = first.as_ref().is_some_and(|arg| arg == "core");
    sikemux_host::panic_reports::record_panics(
        if core { "core" } else { "cli" },
        sikemux_host::BUILD_COMMIT,
    );
    sikemux_keychain::settle(core && !sikemux_host::crash_reports::is_reporter());
    #[cfg(windows)]
    if first
        .as_ref()
        .is_some_and(|arg| arg == sikemux_hostkit::login_item::START_CORE_ARG)
    {
        let file = std::env::args_os().nth(2).unwrap_or_default();
        std::process::exit(sikemux_hostkit::login_item::keep_core_up(file.as_ref()));
    }
    if core {
        use sikemux_host::crash_reports::{self, Process};
        crash_reports::start(Process::Core, core_mode::data_dir(), core_mode::socket());
    }
    if let Some(code) = sikemux_core::git_guard::shim::run_if_git() {
        std::process::exit(code);
    }
    if core {
        let code = core_mode::run();
        sikemux_host::crash_reports::stop();
        std::process::exit(code);
    }
    let code = if first.is_some_and(|arg| arg == cli_client::TOOLS_MCP_FLAG) {
        let gateway = std::env::args_os()
            .skip(2)
            .any(|arg| arg == cli_client::TOOLS_MCP_GATEWAY_FLAG);
        tools_mcp::run(gateway)
    } else {
        cli_client::run()
    };
    std::process::exit(code);
}
