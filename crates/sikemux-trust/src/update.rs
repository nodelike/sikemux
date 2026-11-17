//! The service's half of an update. The installer the core downloaded sits in a folder the
//! person's programs can write, so the service copies it into its own, checks that copy against
//! the release key and runs the same copy. Once the installer starts the new service, it opens
//! the app again in the session that asked.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows_sys::Win32::System::RemoteDesktop::WTSQueryUserToken;

use crate::service::{quote, system_folder};
use crate::spawn::{spawn, Start};
use crate::token::{Integrity, Token};

const LARGEST: u64 = 1 << 30;
/// A mark older than this belongs to an install that never started the service again.
const REOPEN_WITHIN: Duration = Duration::from_secs(10 * 60);
const APP: &str = "sikemux.exe";
/// One update at a time, since each empties the same folder.
static APPLYING: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Reopen {
    user: String,
    session: u32,
}

/// Runs the release's installer for `version`, read from `setup` with the asking person's
/// rights, once its copy matches `signature` and it is newer than this service.
pub(crate) fn apply(
    person: &Token,
    key: &PublicKey,
    setup: &Path,
    signature: &str,
    version: &str,
) -> Result<(), String> {
    let version = Version::parse(version.trim_start_matches('v'))
        .map_err(|error| format!("the update's version: {error}"))?;
    let own = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| error.to_string())?;
    if version <= own {
        return Err(format!("Sikemux {own} is already as new as {version}"));
    }
    let _applying = APPLYING.lock().unwrap_or_else(|held| held.into_inner());
    let source = person
        .as_user(|| File::open(setup))
        .and_then(|opened| opened)
        .map_err(|error| format!("the installer: {error}"))?;
    let folder = updates()?;
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let copy = folder.join(format!("Sikemux_{version}-setup.exe"));
    let mut written = File::create_new(&copy).map_err(|error| error.to_string())?;
    let copied =
        io::copy(&mut source.take(LARGEST + 1), &mut written).map_err(|error| error.to_string())?;
    drop(written);
    if copied > LARGEST {
        return Err("the installer is larger than any release would be".into());
    }
    // Held, with no one else let in to write, until the installer has started from it.
    let mut held = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&copy)
        .map_err(|error| error.to_string())?;
    let comment = verified(&mut held, key, signature)?;
    if !signed_for(&comment, &version) {
        return Err(format!(
            "the signature was not made for Sikemux {version}'s installer"
        ));
    }
    let mark = Reopen {
        user: person.user().map_err(|error| error.to_string())?,
        session: person.session().map_err(|error| error.to_string())?,
    };
    let mark_file = reopen_mark()?;
    fs::write(
        &mark_file,
        serde_json::to_vec(&mark).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let started = run_installer(&copy, &folder);
    drop(held);
    if let Err(error) = started {
        let _ = fs::remove_file(&mark_file);
        return Err(format!("the installer did not start: {error}"));
    }
    Ok(())
}

fn run_installer(copy: &Path, folder: &Path) -> io::Result<()> {
    let command_line = format!(
        "{} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
        quote(copy.as_os_str()).map_err(io::Error::other)?
    );
    spawn(&Start {
        token: &Token::own()?,
        command_line: &command_line,
        inherit: &[],
        output: None,
        environment: None,
        directory: Some(&folder.to_string_lossy()),
        process_access: None,
        on_desktop: false,
    })?;
    Ok(())
}

/// Opens the app again for the person whose update the installer just put in place.
pub(crate) fn reopen_app(program: &Path) {
    let Ok(folder) = updates() else {
        return;
    };
    let _ = fs::remove_dir_all(folder);
    let Ok(mark_file) = reopen_mark() else {
        return;
    };
    let fresh = fs::metadata(&mark_file)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age < REOPEN_WITHIN));
    let mark = fs::read(&mark_file);
    let _ = fs::remove_file(&mark_file);
    let Some(mark) = mark
        .ok()
        .filter(|_| fresh)
        .and_then(|bytes| serde_json::from_slice::<Reopen>(&bytes).ok())
    else {
        return;
    };
    if let Err(error) = open_app(program, &mark) {
        eprintln!("the app could not open again after the update: {error}");
    }
}

fn open_app(program: &Path, mark: &Reopen) -> io::Result<()> {
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: the call writes one handle, owned by the token below, or fails.
    if unsafe { WTSQueryUserToken(mark.session, &mut handle) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Token::owning(handle);
    if token.user()? != mark.user {
        return Err(io::Error::other(
            "someone else is signed in to that session now",
        ));
    }
    if token.integrity()? != Integrity::Medium {
        return Err(io::Error::other("the session's token is not at Medium"));
    }
    let folder = program
        .parent()
        .ok_or_else(|| io::Error::other("the service has no folder"))?;
    let command_line = quote(folder.join(APP).as_os_str()).map_err(io::Error::other)?;
    spawn(&Start {
        token: &token,
        command_line: &command_line,
        inherit: &[],
        output: None,
        environment: None,
        directory: Some(&folder.to_string_lossy()),
        process_access: None,
        on_desktop: true,
    })?;
    Ok(())
}

fn verified(file: &mut File, key: &PublicKey, signature: &str) -> Result<String, String> {
    let text = base64::engine::general_purpose::STANDARD
        .decode(signature.trim())
        .map_err(|error| format!("the update's signature: {error}"))?;
    let signature = Signature::decode(&String::from_utf8_lossy(&text))
        .map_err(|error| format!("the update's signature: {error}"))?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|_| "the update is signed with another key".to_owned())?;
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        verifier.update(&chunk[..read]);
    }
    verifier
        .finalize()
        .map_err(|_| "the installer does not match the release's signature".to_owned())?;
    Ok(signature.trusted_comment().to_owned())
}

/// The release scripts sign `file:Sikemux_<version>_<arch>-setup.exe` and `version:<version>`.
fn signed_for(comment: &str, version: &Version) -> bool {
    let fields: Vec<&str> = comment.split('\t').collect();
    let file = format!("file:Sikemux_{version}_");
    fields.contains(&format!("version:{version}").as_str())
        && fields
            .iter()
            .any(|field| field.starts_with(&file) && field.ends_with("-setup.exe"))
}

/// Inside the service's own profile, which only it and administrators can write.
fn own_folder() -> io::Result<PathBuf> {
    Ok(system_folder()?.join(r"config\systemprofile\AppData\Local\Sikemux"))
}

fn updates() -> Result<PathBuf, String> {
    own_folder()
        .map(|folder| folder.join("update"))
        .map_err(|error| error.to_string())
}

fn reopen_mark() -> Result<PathBuf, String> {
    let folder = own_folder().map_err(|error| error.to_string())?;
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    Ok(folder.join("reopen.json"))
}
