//! What root does to update the core: find the newest release on the
//! person's channel in the feeds, check Sikemux signed it for this very file
//! and version before unpacking or running any of it, and put it where the
//! installer does, setgid to the group that guards the core's secrets. From
//! whoever asked it takes only who they are and which channel they follow.

use std::ffi::CStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};
use sikemux_core::host_update::{self, HostRequest, UpdateStep};
use sikemux_core::protocol::UpdateChannel;
use sikemux_core::release_feed;

const CORE: &str = "sikemux-host";
const GROUP: &CStr = c"sikemux";
const MAX_REQUEST_BYTES: u64 = 256;
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
    let last = match (peer_uid(io::stdin().as_raw_fd()), request()) {
        (Some(person), Ok(request)) => install(person, request.channel, &mut send),
        (None, _) => UpdateStep::Failed {
            message: format!("run this through {}", host_update::SOCKET),
        },
        (_, Err(message)) => UpdateStep::Failed { message },
    };
    send(&last);
    i32::from(matches!(last, UpdateStep::Failed { .. }))
}

fn request() -> Result<HostRequest, String> {
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
    report: &mut dyn FnMut(&UpdateStep),
) -> UpdateStep {
    let attempt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
        .and_then(|runtime| runtime.block_on(attempt(person, channel, report)));
    attempt.unwrap_or_else(|message| UpdateStep::Failed { message })
}

async fn attempt(
    person: u32,
    channel: UpdateChannel,
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
    if newest.version <= installed {
        write_checked(dir, channel, &installed, inode);
        return Ok(current);
    }
    let asset = newest
        .feed
        .platforms
        .get(&release_feed::host_platform())
        .cloned()
        .ok_or_else(|| format!("{} has no core for this machine", newest.version))?;

    let work = dir.join(".update");
    let _ = fs::remove_dir_all(&work);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&work)
        .map_err(|error| error.to_string())?;
    let result = fetch_and_place(core, &work, &newest.version, &asset, report).await;
    let _ = fs::remove_dir_all(&work);
    let inode = result?;
    write_checked(dir, channel, &newest.version, inode);
    Ok(UpdateStep::Installed {
        from: installed.to_string(),
        version: newest.version.to_string(),
    })
}

/// Downloads the release, checking the updater key's signature as it comes
/// and that it was made for this very file and version, so no other
/// release's signature can stand in for it.
async fn fetch_and_place(
    core: &Path,
    work: &Path,
    version: &Version,
    asset: &release_feed::Asset,
    report: &mut dyn FnMut(&UpdateStep),
) -> Result<u64, String> {
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
    place(&archive, core)
}

/// Unpacks the core beside the one it replaces and moves it into place,
/// answering with its inode. The archive's signature is checked already.
fn place(archive: &Path, target: &Path) -> Result<u64, String> {
    let failed = |error: io::Error| format!("could not install the core: {error}");
    let work = archive.parent().ok_or("the download has no folder")?;
    let untarred = sikemux_process::user_environment::command("/usr/bin/tar")
        .args(["--no-same-owner", "--no-same-permissions", "-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(work)
        .arg(CORE)
        .status()
        .map_err(failed)?;
    let unpacked = work.join(CORE);
    if !untarred.success() || !unpacked.is_file() {
        return Err(format!("the release has no {CORE} in it"));
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
        return Err(failed(error));
    }
    let gid = group_id()?;
    let fd = core.as_raw_fd();
    // SAFETY: fd is the open file above; fchown and fchmod only change its
    // owner and mode. The mode is set after the owner, which clears setgid.
    let owned = unsafe { libc::fchown(fd, 0, gid) == 0 && libc::fchmod(fd, 0o2755) == 0 };
    if !owned {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(&staged);
        return Err(failed(error));
    }
    core.sync_all().map_err(failed)?;
    let inode = core.metadata().map_err(failed)?.ino();
    drop(core);
    fs::rename(&staged, target).map_err(failed)?;
    if let Ok(dir) = File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(inode)
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
fn peer_uid(fd: i32) -> Option<u32> {
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
