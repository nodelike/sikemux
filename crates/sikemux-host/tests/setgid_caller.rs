//! On Linux the installer makes `sikemux-host` setgid, and an agent's tools
//! server is that same program. The core must still tell which agent a call
//! from it came from: temple's agents once got "trigger_add needs the calling
//! agent's id" on every call because the core could not see the server's sockets.

#![cfg(target_os = "linux")]

use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Stdio;

use sikemux_agents::callers;

fn other_group() -> Option<libc::gid_t> {
    // SAFETY: a zero count asks only how many groups there are.
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = vec![0; usize::try_from(count).ok()?];
    // SAFETY: `groups` holds `count` writable ids.
    let count = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
    groups.truncate(usize::try_from(count).ok()?);
    // SAFETY: getgid takes nothing and cannot fail.
    let own = unsafe { libc::getgid() };
    groups.into_iter().find(|group| *group != own)
}

fn ignores_setgid(folder: &std::path::Path) -> bool {
    let path = std::ffi::CString::new(folder.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: statvfs is plain numbers, for which all zeroes is a value.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a NUL-terminated string and `stats` a writable statvfs.
    let read = unsafe { libc::statvfs(path.as_ptr(), &mut stats) } == 0;
    read && stats.f_flag & libc::ST_NOSUID != 0
}

#[test]
fn a_setgid_tools_server_is_known_as_its_agent() {
    let Some(group) = other_group() else {
        eprintln!("skipped: this user belongs to no second group to make the server setgid to");
        return;
    };
    let folder = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    if ignores_setgid(folder.path()) {
        eprintln!("skipped: the temporary folder's volume ignores setgid");
        return;
    }
    let program = folder.path().join("sikemux-host");
    std::fs::copy(env!("CARGO_BIN_EXE_sikemux-host"), &program).unwrap();
    std::os::unix::fs::chown(&program, None, Some(group)).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o2755)).unwrap();
    assert_eq!(std::fs::metadata(&program).unwrap().gid(), group);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let endpoint = folder.path().join("cli.json");
    let descriptor = serde_json::json!({
        "protocol": sikemux_core::cli::protocol::CLI_PROTOCOL_VERSION,
        "pid": std::process::id(),
        "port": port,
        "token": "test-token",
        "version": "0.0.0",
    });
    std::fs::File::create(&endpoint)
        .unwrap()
        .write_all(descriptor.to_string().as_bytes())
        .unwrap();

    let mut server = sikemux_process::user_environment::command(&program)
        .arg("--tools-mcp")
        .env(sikemux_process::vars::TOOLS_AGENT_ID, "agent-setgid")
        .env("SIKEMUX_CLI_ENDPOINT", &endpoint)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    callers::started(server.id(), "agent-setgid");

    let (_connection, peer) = listener.accept().unwrap();
    let named = callers::identify(peer, port);
    let _ = server.kill();
    let _ = server.wait();
    assert_eq!(named.as_deref(), Some("agent-setgid"));
}
