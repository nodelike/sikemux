use std::path::{Path, PathBuf};

/// `sikemux core`: runs the background process that owns terminals in the
/// foreground, as the service and the app start it. See
/// [`sikemux_core::server::main`]; the commands that manage it are in
/// [`sikemux_host::headless`].
pub fn run() -> i32 {
    sikemux_process::cgroups::take_own_unit();
    let args: Vec<String> = std::env::args().skip(2).collect();
    sikemux_host::cli_paths::link_cli_for_children();
    let plugins = data_dir().and_then(|data_dir| sikemux_host::provide_to_core(&data_dir));
    if !args
        .iter()
        .any(|arg| arg == sikemux_core::frozen::UPGRADE_INFO_ARG)
    {
        rewrite_service();
    }
    sikemux_core::server::main(args.into_iter(), sikemux_host::build_identity(), plugins)
}

/// The folder `--data-dir` names.
pub fn data_dir() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(2);
    args.find(|arg| arg == "--data-dir")?;
    args.next().map(PathBuf::from)
}

/// A core that updated itself starts its service the way its own release
/// would, as `sikemux enable` does after an install.
fn rewrite_service() {
    use sikemux_host::login_item::{self, IDENTIFIER};
    let installed = std::env::current_exe()
        .is_ok_and(|exe| exe == Path::new(sikemux_core::host_update::INSTALLED_CORE));
    if !installed || !login_item::path(IDENTIFIER).is_some_and(|path| path.exists()) {
        return;
    }
    if let Some(launch) = login_item::headless_launch() {
        if let Err(error) = login_item::install(IDENTIFIER, &launch) {
            eprintln!("sikemux core: could not rewrite its service: {error}");
        }
    }
}
