//! What git runs to ask for a credential for a remote with an identity of the agents' own:
//! this binary under another name, which asks the core for one that lasts one command.
//!
//! Git pushes to such a remote go to it too, since no token an agent can reach may write the
//! repository. It has the core make a bare repository of its own, the outbox, and serves the
//! push from there; the outbox's hook then asks the core to forward exactly what was pushed.

use std::ffi::OsString;
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use sikemux_process::vars;

const ANSWER_WAIT: Duration = Duration::from_secs(30);
const FORWARD_WAIT: Duration = Duration::from_secs(30 * 60);

/// Answers git's `get` with the core at the endpoint git names first. `store` and `erase`
/// keep nothing, as the core keeps no credential it handed out.
pub(super) fn run(args: Vec<OsString>) -> i32 {
    let mut args = args.into_iter();
    let (Some(endpoint), Some(action)) = (args.next(), args.next()) else {
        return 0;
    };
    let endpoint = Path::new(&endpoint);
    if action == "outbox" {
        return outbox(endpoint, args.next(), args.next());
    }
    if action == "forward" {
        return forward(endpoint);
    }
    let asked = read_request();
    if action != "get" {
        return 0;
    }
    let (Some("https"), Some(host), Some(path)) =
        (asked("protocol").as_deref(), asked("host"), asked("path"))
    else {
        return 0;
    };
    let Some(answer) = ask(endpoint, &format!("https://{host}/{path}")) else {
        return 0;
    };
    let (Some(username), Some(password)) =
        (answer["username"].as_str(), answer["password"].as_str())
    else {
        return 0;
    };
    let mut reply = format!("username={username}\npassword={password}\n");
    if let Some(expires) = answer["expiresAt"].as_u64() {
        reply.push_str(&format!("password_expiry_utc={}\n", expires / 1000));
    }
    let _ = std::io::stdout().write_all(reply.as_bytes());
    0
}

/// The core's answer for `remote`, an `https` address: a username and password, or nothing.
pub(super) fn ask(endpoint: &Path, remote: &str) -> Option<Value> {
    let cwd = std::env::current_dir().ok()?;
    let mut params = json!({ "remote": remote });
    if let Some(grant) = std::env::var_os(vars::PUSH_GRANT) {
        params["grant"] = json!(grant.to_string_lossy());
    }
    crate::cli::client::attempt(
        endpoint,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::GIT_CREDENTIAL_METHOD,
        &params,
        ANSWER_WAIT,
    )
    .ok()
}

/// Serves git's push to `remote` from an outbox the core makes for it, and has the core take
/// it away once the push is done.
fn outbox(endpoint: &Path, service: Option<OsString>, remote: Option<OsString>) -> i32 {
    let (Some(service), Some(remote)) = (service, remote) else {
        return 1;
    };
    if service != "git-receive-pack" {
        eprintln!("Sikemux: only pushes go through the core to this remote");
        return 1;
    }
    let opened = call(
        endpoint,
        &json!({ "step": "open", "remote": remote.to_string_lossy() }),
        FORWARD_WAIT,
    );
    let Some(outbox) = opened["outbox"].as_str() else {
        eprintln!("{}", said(&opened));
        return 1;
    };
    let Some(git) = super::shim::real_git() else {
        eprintln!("git: Sikemux found no git to run on PATH");
        return 1;
    };
    let status = sikemux_process::user_environment::command(&git)
        .env_clear()
        .envs(std::env::var_os("PATH").map(|path| ("PATH", path)))
        .env("HOME", outbox)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["receive-pack", outbox])
        .status();
    call(
        endpoint,
        &json!({ "step": "close", "outbox": outbox }),
        ANSWER_WAIT,
    );
    status.ok().and_then(|status| status.code()).unwrap_or(1)
}

/// The outbox's pre-receive hook: has the core forward the ref updates git hands it on stdin,
/// and passes on what the remote said.
fn forward(endpoint: &Path) -> i32 {
    let mut lines = String::new();
    let _ = std::io::stdin().read_to_string(&mut lines);
    let updates: Vec<Value> = lines
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (old, new, name) = (words.next()?, words.next()?, words.next()?);
            Some(json!({ "old": old, "new": new, "ref": name }))
        })
        .collect();
    let Ok(outbox) = std::env::current_dir() else {
        return 1;
    };
    let mut params = json!({
        "step": "forward",
        "outbox": outbox.to_string_lossy(),
        "updates": updates,
    });
    if let Some(quarantine) = std::env::var_os("GIT_QUARANTINE_PATH") {
        params["quarantine"] = json!(quarantine.to_string_lossy());
    }
    let answer = call(endpoint, &params, FORWARD_WAIT);
    let said = said(&answer);
    if !said.is_empty() {
        eprintln!("{said}");
    }
    match answer["forwarded"].as_bool() {
        Some(true) => 0,
        _ => 1,
    }
}

fn call(endpoint: &Path, params: &Value, wait: Duration) -> Value {
    let cwd = std::env::current_dir().unwrap_or_default();
    crate::cli::client::attempt(
        endpoint,
        &cwd.to_string_lossy(),
        None,
        crate::cli::protocol::GIT_OUTBOX_METHOD,
        params,
        wait,
    )
    .unwrap_or_else(|(crate::cli::client::Failure::NotSent(said)
    | crate::cli::client::Failure::Sent(said))| json!({ "said": said }))
}

fn said(answer: &Value) -> String {
    answer["said"]
        .as_str()
        .unwrap_or_default()
        .trim_end()
        .to_owned()
}

/// git's `key=value` lines on stdin, up to the blank line that ends them.
fn read_request() -> impl Fn(&str) -> Option<String> {
    let lines: Vec<(String, String)> = std::io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect();
    move |key| {
        lines
            .iter()
            .find(|(found, _)| found == key)
            .map(|(_, value)| value.clone())
    }
}
