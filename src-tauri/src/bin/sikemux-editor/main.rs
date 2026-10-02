//! The `sikemux` command and `$EDITOR` inside Sikemux terminals. Agents start
//! the same binary with `--tools-mcp` to reach their browser and workspace tools,
//! and `sikemux core` runs the background process that owns terminals.

#[cfg(unix)]
mod core_mode;
mod tools_mcp;

use sikemux_lib::cli_client;

fn main() {
    let first = std::env::args_os().nth(1);
    #[cfg(unix)]
    if first.as_ref().is_some_and(|arg| arg == "core") {
        std::process::exit(core_mode::run());
    }
    let code = if first.is_some_and(|arg| arg == cli_client::TOOLS_MCP_FLAG) {
        tools_mcp::run()
    } else {
        cli_client::run()
    };
    std::process::exit(code);
}
