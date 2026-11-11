//! What root does to update the core, and the app unpacked beside it: find
//! the newest release on the person's channel in the feeds, check Sikemux
//! signed it for this very file and version before unpacking or running any
//! of it, and put it where the installer does, setgid to the group that guards
//! the core's secrets. From whoever asked it takes only who they are and which
//! channel they follow.

use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};
use sikemux_core::host_update::{self, HostAction, HostRequest, UpdateStep};
use sikemux_core::release_feed;
use sikemux_view::prefs::UpdateChannel;
use sikemux_view::updates::{UpdateOutcome, UpdateOutcomeState};

const CORE: &str = sikemux_process::names::CORE_PROGRAM;
const GROUP: &CStr = c"sikemux";
const MAX_REQUEST_BYTES: u64 = 4096;
/// How long a caller has to say what it wants, so idle connections cannot hold the service.
const REQUEST_WAIT: libc::timeval = libc::timeval {
    tv_sec: 10,
    tv_usec: 0,
};
/// How much of a failure's output an outcome keeps as evidence.
const EVIDENCE_LINES: usize = 20;
const MAX_ARCHIVE_BYTES: u64 = 256 << 20;
const MAX_CORE_BYTES: u64 = 512 << 20;
/// However often it is asked, root reads the feeds at most this often.
const RECHECK: Duration = Duration::from_secs(5 * 60);

/// What root last found, and the file it was about, so a core the installer
/// put there since is read afresh.
#[derive(Serialize, Deserialize)]
struct Checked {
    at: u64,
    channel: UpdateChannel,
    installed: String,
    inode: u64,
}

/// The update the socket's service runs for whoever connected to it,
/// answering on the same connection.
pub fn serve() -> i32 {
    let mut answer = io::stdout().lock();
    let mut send = |step: &UpdateStep| {
        if let Ok(line) = serde_json::to_string(step) {
            let _ = writeln!(answer, "{line}");
            let _ = answer.flush();
        }
    };
    let last = match (peer(io::stdin().as_raw_fd()), request()) {
        (Some(person), Ok(request)) if request.action == HostAction::Install => {
            install(person, request.channel, request.id, &mut send)
        }
        (Some(person), Ok(request)) => settle(request, person),
        (None, _) => UpdateStep::Failed {
            message: format!("run this through {}", host_update::SOCKET),
        },
        (_, Err(message)) => UpdateStep::Failed { message },
    };
    if let UpdateStep::Failed { message } = &last {
        eprintln!("sikemux update: {message}");
    }
    send(&last);
    i32::from(matches!(last, UpdateStep::Failed { .. }))
}

fn request() -> Result<HostRequest, String> {
    let wait = REQUEST_WAIT;
    // SAFETY: SO_RCVTIMEO reads one timeval, valid for its size, for the socket on stdin.
    unsafe {
        libc::setsockopt(
            io::stdin().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const wait).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
    let mut line = String::new();
    io::stdin()
        .lock()
        .take(MAX_REQUEST_BYTES)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&line).map_err(|error| format!("an unreadable request: {error}"))
}

/// Installs the newest release on `channel` for `person`, unless they turned
/// updates off.
pub fn install(
    person: u32,
    channel: UpdateChannel,
    id: Option<String>,
    report: &mut dyn FnMut(&UpdateStep),
) -> UpdateStep {
    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let attempt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|runtime| runtime.block_on(attempt(person, channel, &id, report)));
    attempt.unwrap_or_else(|message| UpdateStep::Failed { message })
}

async fn attempt(
    person: u32,
    channel: UpdateChannel,
    id: &str,
    report: &mut dyn FnMut(&UpdateStep),
) -> Result<UpdateStep, String> {
    // SAFETY: geteuid takes nothing and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Err("only root can install the core".into());
    }
    let switch = sikemux_keychain::read_guarded_of(
        person,
        host_update::SWITCH_SERVICE,
        host_update::SWITCH_ACCOUNT,
    )
    .map_err(|error| error.to_string())?;
    if switch.as_deref() == Some(host_update::SWITCH_OFF) {
        return Ok(UpdateStep::Off);
    }
    let core = Path::new(host_update::INSTALLED_CORE);
    let dir = core.parent().ok_or("the core has no folder")?;
    let _lock = lock(dir)?;
    let inode = fs::metadata(core)
        .map_err(|error| format!("{} cannot be read: {error}", core.display()))?
        .ino();
    let checked = read_checked(dir).filter(|checked| checked.inode == inode);
    let installed = match &checked {
        Some(checked) => Version::parse(&checked.installed).map_err(|error| error.to_string())?,
        None if same_file(core) => own_version()?,
        None => return Err(format!("{} is not the core running this", core.display())),
    };
    let current = UpdateStep::Current {
        version: installed.to_string(),
    };
    if checked
        .as_ref()
        .is_some_and(|checked| checked.channel == channel && now() < checked.at + RECHECK.as_secs())
    {
        return Ok(current);
    }
    let newest = release_feed::newest(channel).await?;
    let rolled_back = host_update::read_outcome().is_some_and(|outcome| {
        outcome.state == UpdateOutcomeState::RolledBack
            && outcome.to.as_deref() == Some(&newest.version.to_string())
    });
    if newest.version <= installed || rolled_back {
        write_checked(dir, channel, &installed, inode);
        return Ok(current);
    }
    let mut outcome = UpdateOutcome {
        id: id.to_owned(),
        state: UpdateOutcomeState::Failed,
        from: installed.to_string(),
        to: Some(newest.version.to_string()),
        at: now_ms(),
        reason: Some("the update service stopped before it finished".into()),
        evidence: None,
    };
    forget_previous(dir);
    write_outcome(&outcome);
    let asset = newest
        .feed
        .platforms
        .get(&release_feed::host_platform())
        .cloned()
        .ok_or_else(|| format!("{} has no core for this machine", newest.version))?;
    let app = Path::new(host_update::INSTALLED_APP);
    let app_asset = app
        .is_dir()
        .then(|| {
            newest
                .feed
                .platforms
                .get(&release_feed::platform())
                .cloned()
                .ok_or_else(|| format!("{} has no app for this machine", newest.version))
        })
        .transpose()?;

    let work = dir.join(".update");
    let _ = fs::remove_dir_all(&work);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&work)
        .map_err(|error| error.to_string())?;
    let result: Result<u64, Failure> = async {
        let unpacked = match &app_asset {
            Some(app_asset) => Some(fetch_app(&work, &newest.version, app_asset, report).await?),
            None => None,
        };
        let archive = fetch_core(&work, &newest.version, &asset, report).await?;
        // The app goes in first: should the core then fail, the next try finds the old
        // version still installed and puts both in again.
        if let Some(unpacked) = unpacked {
            replace_app(&unpacked, app)?;
        }
        place(&archive, core)
    }
    .await;
    let _ = fs::remove_dir_all(&work);
    let inode = match result {
        Ok(inode) => inode,
        Err(failure) => {
            outcome.at = now_ms();
            outcome.reason = Some(failure.message.clone());
            outcome.evidence = failure.evidence;
            write_outcome(&outcome);
            return Err(failure.message);
        }
    };
    write_checked(dir, channel, &newest.version, inode);
    outcome.state = UpdateOutcomeState::Pending;
    outcome.at = now_ms();
    outcome.reason = None;
    write_waiting(
        dir,
        &Waiting {
            id: id.to_owned(),
            person,
        },
    );
    write_outcome(&outcome);
    Ok(UpdateStep::Installed {
        from: installed.to_string(),
        version: newest.version.to_string(),
    })
}

/// Downloads the release, checking the updater key's signature as it comes
/// and that it was made for this very file and version, so no other
/// release's signature can stand in for it.
async fn fetch_core(
    work: &Path,
    version: &Version,
    asset: &release_feed::Asset,
    report: &mut dyn FnMut(&UpdateStep),
) -> Result<PathBuf, String> {
    let file = host_update::host_asset(version, std::env::consts::ARCH);
    let archive = work.join(&file);
    let key = release_feed::release_key()?;
    let mut bytes = 0;
    let signed = release_feed::fetch_verified(
        &asset.url,
        &asset.signature,
        &key,
        &archive,
        Some(MAX_ARCHIVE_BYTES),
        |done, _| bytes = done,
    )
    .await?;
    report(&UpdateStep::Downloaded {
        version: version.to_string(),
        bytes,
    });
    if !release_feed::signed_for(&signed, &file, version) {
        return Err(format!(
            "{file} is not signed by Sikemux: its signature was made for another file ({signed})"
        ));
    }
    report(&UpdateStep::Verified);
    Ok(archive)
}

/// Downloads the release's AppImage, checked as the core is, and unpacks it in
/// `work`, answering with the unpacked folder.
async fn fetch_app(
    work: &Path,
    version: &Version,
    asset: &release_feed::Asset,
    report: &mut dyn FnMut(&UpdateStep),
) -> Result<PathBuf, Failure> {
    let file = format!("Sikemux_{version}_{}.AppImage", package_arch());
    let appimage = work.join(&file);
    let key = release_feed::release_key()?;
    let mut bytes = 0;
    let signed = release_feed::fetch_verified(
        &asset.url,
        &asset.signature,
        &key,
        &appimage,
        Some(MAX_ARCHIVE_BYTES),
        |done, _| bytes = done,
    )
    .await?;
    report(&UpdateStep::Downloaded {
        version: version.to_string(),
        bytes,
    });
    if !release_feed::signed_for(&signed, &file, version) {
        return Err(format!(
            "{file} is not signed by Sikemux: its signature was made for another file ({signed})"
        )
        .into());
    }
    report(&UpdateStep::Verified);
    let failed = |error: io::Error| format!("could not unpack the app: {error}");
    fs::set_permissions(&appimage, fs::Permissions::from_mode(0o700)).map_err(failed)?;
    let extracted = sikemux_process::user_environment::command(&appimage)
        .arg("--appimage-extract")
        .current_dir(work)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(failed)?;
    let unpacked = work.join("squashfs-root");
    if !extracted.status.success() || !unpacked.join(host_update::INSTALLED_APP_PROGRAM).is_file() {
        return Err(Failure {
            message: format!("{file} could not be unpacked ({})", extracted.status),
            evidence: tail(&extracted.stderr),
        });
    }
    fs::set_permissions(&unpacked, fs::Permissions::from_mode(0o755)).map_err(failed)?;
    only_root_writes(&unpacked).map_err(failed)?;
    Ok(unpacked)
}

/// Takes write access away from everyone but root throughout `folder`, as the
/// installer does, since the core trusts whatever runs from it.
fn only_root_writes(folder: &Path) -> io::Result<()> {
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let mode = metadata.permissions().mode();
        if mode & 0o022 != 0 {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode & !0o022))?;
        }
        if metadata.is_dir() {
            only_root_writes(&path)?;
        }
    }
    Ok(())
}

/// What the release calls this machine in its package names.
fn package_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// Puts the unpacked app in place of the installed one, putting the old one back
/// if the new one cannot go in.
fn replace_app(unpacked: &Path, app: &Path) -> Result<(), String> {
    let failed = |error: io::Error| format!("could not install the app: {error}");
    let old = app.with_file_name(".app.old");
    let _ = fs::remove_dir_all(&old);
    fs::rename(app, &old).map_err(failed)?;
    if let Err(error) = fs::rename(unpacked, app) {
        let _ = fs::rename(&old, app);
        return Err(failed(error));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Unpacks the core beside the one it replaces and moves it into place,
/// answering with its inode. The archive's signature is checked already.
fn place(archive: &Path, target: &Path) -> Result<u64, Failure> {
    let failed = |error: io::Error| format!("could not install the core: {error}");
    let work = archive.parent().ok_or("the download has no folder")?;
    let untarred = sikemux_process::user_environment::command("/usr/bin/tar")
        .args(["--no-same-owner", "--no-same-permissions", "-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(work)
        .arg(CORE)
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(failed)?;
    let unpacked = work.join(CORE);
    if !untarred.status.success() || !unpacked.is_file() {
        return Err(Failure {
            message: format!("the release has no {CORE} in it"),
            evidence: tail(&untarred.stderr),
        });
    }
    let dir = target.parent().ok_or("the core has no folder")?;
    let staged = dir.join(".sikemux-host.new");
    let _ = fs::remove_file(&staged);
    let mut core = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&staged)
        .map_err(failed)?;
    let copied =
        File::open(&unpacked).and_then(|from| io::copy(&mut from.take(MAX_CORE_BYTES), &mut core));
    if let Err(error) = copied {
        let _ = fs::remove_file(&staged);
        return Err(failed(error).into());
    }
    let gid = group_id()?;
    let fd = core.as_raw_fd();
    // SAFETY: fd is the open file above; fchown and fchmod only change its
    // owner and mode. The mode is set after the owner, which clears setgid.
    let owned = unsafe { libc::fchown(fd, 0, gid) == 0 && libc::fchmod(fd, 0o2755) == 0 };
    if !owned {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(&staged);
        return Err(failed(error).into());
    }
    core.sync_all().map_err(failed)?;
    let inode = core.metadata().map_err(failed)?.ino();
    drop(core);
    let previous = Path::new(host_update::PREVIOUS_CORE);
    let _ = fs::remove_file(previous);
    let kept = fs::hard_link(target, previous);
    fs::rename(&staged, target).map_err(failed)?;
    match kept {
        Ok(()) => keep_out_of_reach(previous),
        Err(error) => {
            eprintln!("sikemux update: the core in place is not kept to go back to: {error}")
        }
    }
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(inode)
}

/// Why an update failed, with what the tools it ran said.
struct Failure {
    message: String,
    evidence: Option<String>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Failure {
            message,
            evidence: None,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// The last lines of a tool's output, when it said anything.
fn tail(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let kept = lines[lines.len().saturating_sub(EVIDENCE_LINES)..].join("\n");
    (!kept.is_empty()).then_some(kept)
}

/// Settles the update `request` names once its new core has started: drops the
/// core it replaced, or puts that one back. Only the person the update was for may ask, and
/// only through a note their core left in its guarded store, which no other program of theirs
/// can write, so another user cannot undo an update.
fn settle(request: HostRequest, person: u32) -> UpdateStep {
    let failed = |message: &str| UpdateStep::Failed {
        message: message.to_owned(),
    };
    let core = Path::new(host_update::INSTALLED_CORE);
    let Some(dir) = core.parent() else {
        return failed("the core has no folder");
    };
    let _lock = match lock(dir) {
        Ok(lock) => lock,
        Err(message) => return UpdateStep::Failed { message },
    };
    let waiting = read_waiting(dir)
        .filter(|waiting| request.id.as_ref() == Some(&waiting.id) && waiting.person == person);
    let outcome = host_update::read_outcome().filter(|outcome| {
        outcome.state == UpdateOutcomeState::Pending && request.id.as_ref() == Some(&outcome.id)
    });
    let (Some(_), Some(mut outcome)) = (waiting, outcome) else {
        return failed("no update of yours is waiting on that");
    };
    let installed = read_checked(dir).map(|checked| checked.installed);
    let asked = sikemux_keychain::read_guarded_of(
        person,
        host_update::SWITCH_SERVICE,
        host_update::SETTLE_ACCOUNT,
    )
    .ok()
    .flatten();
    if installed != outcome.to
        || asked != Some(host_update::settle_note(&outcome.id, request.action))
    {
        return failed("only the core this update put in place may settle it");
    }
    let previous = Path::new(host_update::PREVIOUS_CORE);
    outcome.at = now_ms();
    let _ = fs::remove_file(dir.join(WAITING));
    if request.action != HostAction::RollBack {
        let _ = fs::remove_file(previous);
        outcome.state = UpdateOutcomeState::Done;
        write_outcome(&outcome);
        return UpdateStep::Current {
            version: outcome.to.unwrap_or(outcome.from),
        };
    }
    let restored = Version::parse(&outcome.from)
        .map_err(|error| error.to_string())
        .and_then(|from| {
            give_back_to_group(previous)?;
            fs::rename(previous, core).map_err(|error| error.to_string())?;
            let inode = fs::metadata(core).map_err(|error| error.to_string())?.ino();
            write_checked(dir, request.channel, &from, inode);
            Ok(())
        });
    if let Err(error) = restored {
        eprintln!("sikemux update: could not put the previous core back: {error}");
        return UpdateStep::Failed {
            message: format!("could not put the previous core back: {error}"),
        };
    }
    let reason = "the new core stopped before it was ready";
    eprintln!("sikemux update: put {} back: {reason}", outcome.from);
    outcome.state = UpdateOutcomeState::RolledBack;
    outcome.reason = Some(reason.into());
    write_outcome(&outcome);
    UpdateStep::Current {
        version: outcome.from,
    }
}

/// Root's own note of whom a pending update is for, which only root reads.
const WAITING: &str = ".update-waiting.json";

#[derive(Serialize, Deserialize)]
struct Waiting {
    id: String,
    person: u32,
}

fn read_waiting(dir: &Path) -> Option<Waiting> {
    serde_json::from_slice(&fs::read(dir.join(WAITING)).ok()?).ok()
}

fn write_waiting(dir: &Path, waiting: &Waiting) {
    if let Ok(bytes) = serde_json::to_vec(waiting) {
        let _ = sikemux_process::files::write_atomic(&dir.join(WAITING), &bytes, true);
    }
}

/// Drops the core an earlier update kept to go back to, as a new update replaces that one.
fn forget_previous(dir: &Path) {
    let _ = fs::remove_file(host_update::PREVIOUS_CORE);
    let _ = fs::remove_file(dir.join(WAITING));
}

/// Makes the kept core root's alone, so nobody runs an old build with the secrets group.
fn keep_out_of_reach(previous: &Path) {
    let kept = std::os::unix::fs::chown(previous, Some(0), Some(0))
        .and_then(|()| fs::set_permissions(previous, fs::Permissions::from_mode(0o700)));
    if let Err(error) = kept {
        eprintln!(
            "sikemux update: the kept core could not be made root's alone, so it goes: {error}"
        );
        let _ = fs::remove_file(previous);
    }
}

/// Gives the kept core back the owner and mode the installed core has, setgid last since
/// changing the owner clears it.
fn give_back_to_group(previous: &Path) -> Result<(), String> {
    let gid = group_id()?;
    std::os::unix::fs::chown(previous, Some(0), Some(gid))
        .and_then(|()| fs::set_permissions(previous, fs::Permissions::from_mode(0o2755)))
        .map_err(|error| error.to_string())
}

/// Records how an update ended, readable by anyone, since the person's core reports it.
fn write_outcome(outcome: &UpdateOutcome) {
    if let Ok(bytes) = serde_json::to_vec(outcome) {
        let path = Path::new(host_update::OUTCOME_FILE);
        if let Err(error) = sikemux_process::files::write_atomic(path, &bytes, false) {
            eprintln!("sikemux update: could not record how the update ended: {error}");
        }
    }
}

fn now_ms() -> u64 {
    sikemux_process::clock::epoch_ms()
}

fn group_id() -> Result<libc::gid_t, String> {
    // SAFETY: GROUP is a valid C string; the record getgrnam returns is read
    // at once, before anything else could overwrite it.
    let group = unsafe { libc::getgrnam(GROUP.as_ptr()) };
    if group.is_null() {
        return Err("the group sikemux is missing; rerun the installer".into());
    }
    // SAFETY: group is not null, so it points at a group record.
    Ok(unsafe { (*group).gr_gid })
}

fn lock(dir: &Path) -> Result<File, String> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(".update.lock"))
        .map_err(|error| error.to_string())?;
    // SAFETY: flock takes the open file's descriptor and touches no memory.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(lock)
}

/// Whether `core` is the file this process runs from.
fn same_file(core: &Path) -> bool {
    match (fs::metadata(core), fs::metadata("/proc/self/exe")) {
        (Ok(core), Ok(own)) => core.dev() == own.dev() && core.ino() == own.ino(),
        _ => false,
    }
}

fn own_version() -> Result<Version, String> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| error.to_string())
}

fn read_checked(dir: &Path) -> Option<Checked> {
    serde_json::from_slice(&fs::read(dir.join(".update.json")).ok()?).ok()
}

fn write_checked(dir: &Path, channel: UpdateChannel, installed: &Version, inode: u64) {
    let checked = Checked {
        at: now(),
        channel,
        installed: installed.to_string(),
        inode,
    };
    if let Ok(bytes) = serde_json::to_vec(&checked) {
        let _ = sikemux_process::files::write_atomic(&dir.join(".update.json"), &bytes, false);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Who is on the other end of the socket, as the kernel says.
fn peer(fd: i32) -> Option<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: credentials and length are valid for the size given, which is
    // what SO_PEERCRED writes.
    let read = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut length,
        )
    } == 0;
    read.then_some(credentials.uid)
}
