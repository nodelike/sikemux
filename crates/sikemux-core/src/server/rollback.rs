//! Putting the previous Mac app back when the core its update brought stops before it is ready.
//! An update keeps the app it replaced beside the new one. When the old core hands itself over
//! to the new build, it first starts a watcher from the kept app, so nothing of the new build
//! has to work for the watcher to run. Should the core stop before the new build says it
//! started well, the watcher puts the kept app back and starts its core, which brings the chats
//! back on their sessions. The new core says it started well by settling the update.

use sikemux_process::command::Stdio;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sikemux_process::clock::epoch_ms;
use sikemux_view::updates::{UpdateOutcome, UpdateOutcomeState};

use super::log_times;
use super::parts::Parts;
use super::updates::previous_bundle;
use super::Core;

/// How often the watcher looks for the core, and how long a core that runs it leaves it to
/// settle the update before the watcher stops watching.
const LOOK_EVERY: Duration = Duration::from_millis(200);
const WATCH_FOR: Duration = Duration::from_secs(2 * 60);
pub(crate) const WATCH_ARG: &str = "--watch-handover";

fn outcome_file() -> Option<PathBuf> {
    sikemux_process::paths::config_file("update-outcome", "json")
}

/// How the last update of this Mac app ended.
pub(crate) fn read_outcome() -> Option<UpdateOutcome> {
    serde_json::from_slice(&std::fs::read(outcome_file()?).ok()?).ok()
}

pub(crate) fn write_outcome(outcome: &UpdateOutcome) {
    let Some(path) = outcome_file() else {
        return;
    };
    let written = serde_json::to_vec(outcome)
        .map_err(std::io::Error::other)
        .and_then(|bytes| sikemux_process::files::write_atomic(&path, &bytes, true));
    if let Err(error) = written {
        eprintln!("sikemux core: could not record how the update ended: {error}");
    }
}

/// The update in place and not yet settled.
fn pending() -> Option<UpdateOutcome> {
    read_outcome().filter(|outcome| outcome.state == UpdateOutcomeState::Pending)
}

/// The app bundle this core runs from.
fn own_bundle() -> Option<PathBuf> {
    let core = std::env::current_exe().ok()?;
    sikemux_process::paths::app_bundle_of_core(&core).map(Path::to_path_buf)
}

/// Before this core hands itself over to the build an update put in its place, starts the
/// watcher from the app that update kept, which is this core's own build.
pub(crate) fn watch_the_handover(core: &Core) {
    let Some(bundle) = own_bundle() else {
        return;
    };
    let Some(previous) = previous_bundle(&bundle).filter(|previous| previous.is_dir()) else {
        return;
    };
    if !pending().is_some_and(|outcome| outcome.from == core.config().build.version) {
        return;
    }
    let config = core.config();
    let mut command = sikemux_process::user_environment::command(
        previous.join(sikemux_process::paths::BUNDLED_CORE),
    );
    command
        .arg("core")
        .arg(WATCH_ARG)
        .arg(&bundle)
        .arg("--socket")
        .arg(&config.socket)
        .args(launch_args(
            config.cli_endpoint.as_deref(),
            config.data_dir.as_deref(),
            config.idle_exit,
        ))
        .stdin(Stdio::null())
        .stderr(log_times::log_file().map_or_else(Stdio::inherit, Stdio::from))
        .process_group(0);
    // SAFETY: runs in the forked child before exec and only closes descriptors, which is
    // async-signal-safe. The watcher must not hold the core's lock or terminals open, or it
    // would never see the core stop.
    unsafe {
        command.pre_exec(|| {
            let last = libc::getdtablesize();
            for fd in 3..last {
                libc::close(fd);
            }
            Ok(())
        });
    }
    match command.spawn() {
        Ok(watcher) => eprintln!(
            "sikemux core: watching the hand-over to the update from pid {}",
            watcher.id()
        ),
        Err(error) => eprintln!("sikemux core: the hand-over goes unwatched: {error}"),
    }
}

/// What a core is started with after its socket.
fn launch_args(
    cli_endpoint: Option<&Path>,
    data_dir: Option<&Path>,
    idle_exit: Duration,
) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(endpoint) = cli_endpoint {
        args.extend([OsString::from("--cli-endpoint"), endpoint.into()]);
    }
    if let Some(data_dir) = data_dir {
        args.extend([OsString::from("--data-dir"), data_dir.into()]);
    }
    args.extend([
        OsString::from("--idle-exit-ms"),
        idle_exit.as_millis().to_string().into(),
    ]);
    args
}

/// What the watcher is told: the app it watches, and how to start a core in its place.
pub(crate) struct Watch {
    pub bundle: PathBuf,
    pub socket: PathBuf,
    pub cli_endpoint: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub idle_exit: Duration,
}

/// `sikemux core --watch-handover <app>`: waits while the update is unsettled, and puts the
/// previous app back once no core holds the socket. Returns the exit code.
pub(crate) fn watch(watch: Watch) -> i32 {
    let Some(id) = pending().map(|outcome| outcome.id) else {
        return 0;
    };
    let began = Instant::now();
    let mut running_since: Option<Instant> = None;
    loop {
        let still = pending().filter(|outcome| outcome.id == id);
        let Some(outcome) = still else {
            return 0;
        };
        match sikemux_corekit::lock::lock_holder(&watch.socket) {
            Some(_) => {
                let since = *running_since.get_or_insert_with(Instant::now);
                if since.elapsed() > WATCH_FOR {
                    eprintln!("sikemux core: the update has run {}s without settling; no longer watching it", WATCH_FOR.as_secs());
                    return 0;
                }
            }
            None => return put_back(&watch, outcome, began.elapsed()),
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

fn put_back(watch: &Watch, mut outcome: UpdateOutcome, after: Duration) -> i32 {
    let reason = format!("it stopped {}s after taking over", after.as_secs());
    let Some(previous) = previous_bundle(&watch.bundle) else {
        return 1;
    };
    if let Err(error) = swap_back(&watch.bundle, &previous) {
        eprintln!("sikemux core: could not put {} back: {error}", outcome.from);
        outcome.state = UpdateOutcomeState::Failed;
        outcome.reason = Some(format!(
            "{reason}, and the previous app could not be put back: {error}"
        ));
        outcome.at = epoch_ms();
        write_outcome(&outcome);
        return 1;
    }
    eprintln!("sikemux core: put {} back: {reason}", outcome.from);
    outcome.state = UpdateOutcomeState::RolledBack;
    outcome.reason = Some(reason);
    outcome.at = epoch_ms();
    write_outcome(&outcome);
    let core = watch.bundle.join(sikemux_process::paths::BUNDLED_CORE);
    let log = own_log().unwrap_or_else(|| PathBuf::from("/dev/null"));
    let args = launch_args(
        watch.cli_endpoint.as_deref(),
        watch.data_dir.as_deref(),
        watch.idle_exit,
    );
    match sikemux_corekit::client::ensure_running(&watch.socket, &core, &log, &args) {
        Ok(hello) => {
            eprintln!(
                "sikemux core: {} {} answers again (pid {})",
                hello.build.version, hello.build.commit, hello.pid
            );
            0
        }
        Err(error) => {
            eprintln!("sikemux core: the previous core did not start: {error}");
            1
        }
    }
}

/// Moves the failed app aside and the kept one into its place, then drops the failed one.
fn swap_back(bundle: &Path, previous: &Path) -> std::io::Result<()> {
    let name = bundle.file_name().unwrap_or_default().to_string_lossy();
    let failed = bundle.with_file_name(format!(".{name}.failed"));
    let _ = std::fs::remove_dir_all(&failed);
    std::fs::rename(bundle, &failed)?;
    if let Err(error) = std::fs::rename(previous, bundle) {
        let _ = std::fs::rename(&failed, bundle);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&failed);
    Ok(())
}

/// The file this process logs to, which the core it starts logs to as well.
fn own_log() -> Option<PathBuf> {
    let mut path = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, which `path` holds.
    if unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_GETPATH, path.as_mut_ptr()) } != 0 {
        return None;
    }
    let end = path.iter().position(|byte| *byte == 0)?;
    let path = PathBuf::from(String::from_utf8_lossy(&path[..end]).into_owned());
    path.is_file().then_some(path)
}

/// Once the core an update put in place has run a while, says it started well: drops the
/// app it replaced, which ends the watcher.
pub(crate) async fn settle(core: Arc<Core>, ready_after: Duration) {
    let version = core.config().build.version.clone();
    if !pending().is_some_and(|outcome| outcome.to.as_deref() == Some(version.as_str())) {
        return;
    }
    tokio::time::sleep(ready_after).await;
    let _ = super::connection::blocking(move || {
        let Some(mut outcome) =
            pending().filter(|outcome| outcome.to.as_deref() == Some(version.as_str()))
        else {
            return Ok(());
        };
        outcome.state = UpdateOutcomeState::Done;
        outcome.at = epoch_ms();
        write_outcome(&outcome);
        if let Some(previous) = own_bundle().as_deref().and_then(previous_bundle) {
            let _ = std::fs::remove_dir_all(previous);
        }
        Ok(())
    })
    .await;
}
