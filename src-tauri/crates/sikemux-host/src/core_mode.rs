use std::path::Path;

/// `sikemux core`: runs the background process that owns terminals in the
/// foreground, as the service and the app start it. See
/// [`sikemux_core::server::main`]; the commands that manage it are in
/// [`sikemux_host::headless`].
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if let Some(data_dir) = args
        .iter()
        .position(|arg| arg == "--data-dir")
        .and_then(|at| args.get(at + 1))
    {
        sikemux_host::provide_to_core(Path::new(data_dir));
    }
    sikemux_core::server::main(args.into_iter(), sikemux_host::build_identity())
}
