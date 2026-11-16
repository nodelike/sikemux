//! `sikemux-host --service run|install|stop|remove`: the Windows service that starts each signed-in
//! person's core at High integrity. Install and remove need an administrator.

use sikemux_trust::service::{self, Core};

pub const SERVICE_ARG: &str = "--service";

const CORE: Core = Core {
    subcommand: "core",
    options: &["--cli-endpoint", "--data-dir"],
    release_key: sikemux_corekit::release_feed::release_key,
};

pub fn run(action: Option<&str>) -> i32 {
    let done = match action {
        Some("run") => service::run(CORE),
        Some("install") => std::env::current_exe().and_then(|program| {
            service::install(&format!("\"{}\" {SERVICE_ARG} run", program.display()))
        }),
        Some("stop") => service::stop(),
        Some("remove") => service::remove(),
        _ => {
            eprintln!("usage: sikemux-host {SERVICE_ARG} run|install|stop|remove");
            return 2;
        }
    };
    match done {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("The Sikemux service failed: {error}");
            1
        }
    }
}
