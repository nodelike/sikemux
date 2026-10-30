//! Which agent holds which device. Each claim is a small file named after the
//! device, in a folder every copy of Sikemux on this Mac shares, so a dev build
//! and the installed app never hand one simulator to two agents.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub pid: u32,
    /// The program holding it, so a claim left by a crash is not kept alive by a reused pid.
    pub process: String,
    pub agent_id: String,
    pub project: String,
}

impl Claim {
    pub fn project_name(&self) -> &str {
        Path::new(&self.project)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.project)
    }
}

pub struct Claims {
    dir: PathBuf,
    pid: u32,
}

impl Claims {
    /// The folder the dev build and the installed app both use.
    pub fn shared() -> Self {
        let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
        Self::at(
            home.join("Library/Caches/com.nodelike.sikemux/sim-claims"),
            std::process::id(),
        )
    }

    pub fn at(dir: PathBuf, pid: u32) -> Self {
        Self { dir, pid }
    }

    fn path(&self, udid: &str) -> Option<PathBuf> {
        let plain = !udid.is_empty()
            && udid
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        plain.then(|| self.dir.join(udid))
    }

    fn mine(&self, claim: &Claim, agent_id: &str) -> bool {
        claim.pid == self.pid && claim.agent_id == agent_id
    }

    /// The live claim on a device. One whose program has gone is removed.
    pub fn holder(&self, udid: &str) -> Option<Claim> {
        let path = self.path(udid)?;
        let claim: Claim = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        if claim.pid != self.pid && !running(claim.pid, &claim.process) {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(claim)
    }

    /// Every live claim, by device.
    pub fn all(&self) -> HashMap<String, Claim> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return HashMap::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .filter_map(|udid| Some((udid.clone(), self.holder(&udid)?)))
            .collect()
    }

    /// Claims a device for an agent, or names who already holds it.
    pub fn claim(&self, udid: &str, agent_id: &str, project: &str) -> Result<(), Claim> {
        let Some(path) = self.path(udid) else {
            return Ok(());
        };
        let claim = Claim {
            pid: self.pid,
            process: process_name(self.pid),
            agent_id: agent_id.to_owned(),
            project: project.to_owned(),
        };
        for _ in 0..3 {
            match self.holder(udid) {
                Some(holder) if self.mine(&holder, agent_id) => return Ok(()),
                Some(holder) => return Err(holder),
                None => {}
            }
            if self.write_new(&path, &claim).is_ok() {
                return Ok(());
            }
        }
        self.holder(udid).map_or(Ok(()), Err)
    }

    /// Writes the whole claim beside the target, then links it into place, which fails if another claim got there first.
    fn write_new(&self, path: &Path, claim: &Claim) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        file.write_all(&serde_json::to_vec(claim)?)?;
        std::fs::hard_link(file.path(), path)
    }

    /// Lets go of a device, if this agent holds it.
    pub fn release(&self, udid: &str, agent_id: &str) {
        let Some(path) = self.path(udid) else { return };
        if self
            .holder(udid)
            .is_some_and(|holder| self.mine(&holder, agent_id))
        {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Lets go of everything this copy of Sikemux holds, as it quits.
    pub fn release_all(&self) {
        for (udid, claim) in self.all() {
            if claim.pid == self.pid {
                self.release(&udid, &claim.agent_id);
            }
        }
    }
}

fn process_name(pid: u32) -> String {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its whole length, which is passed with it.
        let written = unsafe {
            libc::proc_name(
                pid as libc::c_int,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
            )
        };
        if written > 0 {
            return String::from_utf8_lossy(&buffer[..written as usize]).into_owned();
        }
    }
    let _ = pid;
    String::new()
}

fn running(pid: u32, process: &str) -> bool {
    let Ok(pid_t) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only asks whether the process exists.
    let exists = unsafe { libc::kill(pid_t, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    exists && (process.is_empty() || process_name(pid) == process)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead_pid() -> u32 {
        let mut child = sikemux_process::user_environment::command("/usr/bin/true")
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[test]
    fn a_device_has_one_holder_until_it_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let claims = Claims::at(dir.path().to_path_buf(), std::process::id());
        claims.claim("U1", "agent-a", "/work/shop").unwrap();
        claims
            .claim("U1", "agent-a", "/work/shop")
            .expect("claiming again is fine");
        let holder = claims.claim("U1", "agent-b", "/work/blog").unwrap_err();
        assert_eq!(holder.agent_id, "agent-a");
        assert_eq!(holder.project_name(), "shop");

        claims.release("U1", "agent-b");
        assert!(claims.holder("U1").is_some(), "only the holder lets go");
        claims.release("U1", "agent-a");
        claims.claim("U1", "agent-b", "/work/blog").unwrap();
        assert_eq!(claims.all()["U1"].agent_id, "agent-b");
    }

    #[test]
    fn another_copy_of_sikemux_sees_the_claim() {
        let dir = tempfile::tempdir().unwrap();
        let release = Claims::at(dir.path().to_path_buf(), std::process::id());
        release.claim("U1", "agent-a", "/work/shop").unwrap();
        let mut other_pid = sikemux_process::user_environment::command("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let dev = Claims::at(dir.path().to_path_buf(), other_pid.id());
        assert_eq!(
            dev.claim("U1", "agent-a", "/work/shop").unwrap_err().pid,
            std::process::id(),
            "the same agent id in another copy is someone else"
        );
        let _ = other_pid.kill();
        let _ = other_pid.wait();
    }

    #[test]
    fn a_claim_left_by_a_program_that_is_gone_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let crashed = Claims::at(dir.path().to_path_buf(), dead_pid());
        crashed.claim("U1", "agent-a", "/work/shop").unwrap();
        let claims = Claims::at(dir.path().to_path_buf(), std::process::id());
        assert_eq!(claims.holder("U1"), None);
        claims.claim("U1", "agent-b", "/work/blog").unwrap();

        let reused = Claim {
            pid: std::process::id(),
            process: "not-this-program".into(),
            agent_id: "agent-c".into(),
            project: "/work/other".into(),
        };
        std::fs::write(dir.path().join("U2"), serde_json::to_vec(&reused).unwrap()).unwrap();
        let elsewhere = Claims::at(dir.path().to_path_buf(), dead_pid());
        assert_eq!(
            elsewhere.holder("U2"),
            None,
            "a pid now used by another program does not keep a claim"
        );
    }

    #[test]
    fn quitting_lets_go_of_only_this_copys_claims() {
        let dir = tempfile::tempdir().unwrap();
        let mut sleeper = sikemux_process::user_environment::command("/bin/sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let other = Claims::at(dir.path().to_path_buf(), sleeper.id());
        let other_claim = Claim {
            pid: sleeper.id(),
            process: String::new(),
            agent_id: "agent-x".into(),
            project: "/work/x".into(),
        };
        std::fs::write(
            dir.path().join("U9"),
            serde_json::to_vec(&other_claim).unwrap(),
        )
        .unwrap();
        let claims = Claims::at(dir.path().to_path_buf(), std::process::id());
        claims.claim("U1", "agent-a", "/work/shop").unwrap();
        claims.release_all();
        assert_eq!(claims.holder("U1"), None);
        assert_eq!(other.holder("U9").unwrap().agent_id, "agent-x");
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }
}
