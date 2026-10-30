// A collection's pre- and post-request scripts and its assertion expressions, run in a
// JavaScript engine on a thread of their own with a time limit, so one that loops cannot hold
// up the host. A script sees only Bruno's script API; a request it makes goes back through the
// host's own send, with the run's trust applied again whatever the script asked for.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_bruno_views::{Check, ScriptLog};

use crate::send::{BruBodyWire, BruSendRequest, BruTrust};

#[cfg(all(target_os = "macos", not(feature = "quickjs")))]
mod jsc;
#[cfg(any(target_os = "linux", feature = "quickjs"))]
mod quickjs;

/// How long one script, or one expression, may run.
const TIME_LIMIT: Duration = Duration::from_secs(15);

const CHAI: &str = include_str!("../../js/chai.js");
const PRELUDE: &str = include_str!("../../js/prelude.js");

/// The request as a script sees and may change it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReqView {
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub name: String,
}

/// The response as a post-response script sees it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResView {
    pub status: u16,
    pub status_text: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub response_time: u64,
}

/// One run's state that scripts read and change: its variables and its request, and what they
/// logged and tested.
pub struct Ctx {
    pub vars: BTreeMap<String, String>,
    pub req: ReqView,
    pub res: Option<ResView>,
    pub logs: Vec<ScriptLog>,
    pub tests: Vec<Check>,
    pub trust: BruTrust,
}

#[derive(Deserialize)]
struct Left {
    vars: BTreeMap<String, String>,
    req: ReqView,
    logs: Vec<ScriptLog>,
    tests: Vec<Check>,
}

/// A request a script asked to send.
#[derive(Deserialize)]
struct Asked {
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<(String, String)>,
    body: BruBodyWire,
}

/// Runs `source` with `input` on a thread of its own, `send` answering the script's requests.
fn evaluate(input: Value, trust: BruTrust) -> Result<String, String> {
    let (done, finished) = mpsc::channel();
    let input = input.to_string();
    let handle = tokio::runtime::Handle::try_current().ok();
    std::thread::Builder::new()
        .name("bruno-script".into())
        .spawn(move || {
            let send =
                move |asked: &str| -> String { send_for_script(asked, &trust, handle.as_ref()) };
            let source = format!("{CHAI}\n{PRELUDE}");
            let _ = done.send(engine(&source, &input, Box::new(send)));
        })
        .map_err(|error| error.to_string())?;
    finished
        .recv_timeout(TIME_LIMIT + Duration::from_secs(5))
        .map_err(|_| "script timed out".to_owned())?
}

#[cfg(all(target_os = "macos", not(feature = "quickjs")))]
fn engine(
    source: &str,
    input: &str,
    send: Box<dyn FnMut(&str) -> String>,
) -> Result<String, String> {
    jsc::run(source, input, send, TIME_LIMIT)
}

#[cfg(any(target_os = "linux", feature = "quickjs"))]
fn engine(
    source: &str,
    input: &str,
    send: Box<dyn FnMut(&str) -> String>,
) -> Result<String, String> {
    quickjs::run(source, input, send, TIME_LIMIT)
}

/// A host with neither engine runs no scripts rather than run them unguarded.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn engine(_: &str, _: &str, _: Box<dyn FnMut(&str) -> String>) -> Result<String, String> {
    Err("scripts are not supported on this host yet".to_owned())
}

/// The request a script asked for, carrying the run's own trust whatever the script passed.
fn request_for_script(asked: &str, trust: &BruTrust) -> Result<BruSendRequest, String> {
    let asked: Asked =
        serde_json::from_str(asked).map_err(|error| format!("bad request: {error}"))?;
    Ok(BruSendRequest {
        method: asked.method,
        url: asked.url,
        headers: asked.headers,
        body: asked.body,
        timeout_ms: 0,
        skip_tls_verify: false,
        trust: trust.clone(),
    })
}

/// Sends what a script asked for and answers it as JSON text.
fn send_for_script(
    asked: &str,
    trust: &BruTrust,
    handle: Option<&tokio::runtime::Handle>,
) -> String {
    let request = match request_for_script(asked, trust) {
        Ok(request) => request,
        Err(error) => return json!({ "error": error }).to_string(),
    };
    let Some(handle) = handle else {
        return json!({ "error": "sending is not available here" }).to_string();
    };
    match handle.block_on(crate::send::send(request)) {
        Ok(response) => json!({ "response": response }).to_string(),
        Err(error) => json!({ "error": error.to_string() }).to_string(),
    }
}

/// Runs a script against the run, which takes up whatever the script changed and logged.
pub async fn run_script(code: &str, ctx: &mut Ctx) {
    if code.trim().is_empty() {
        return;
    }
    let input = json!({
        "code": code,
        "vars": ctx.vars,
        "req": ctx.req,
        "res": ctx.res,
    });
    let trust = ctx.trust.clone();
    let ran = tokio::task::spawn_blocking(move || evaluate(input, trust))
        .await
        .map_err(|error| error.to_string())
        .and_then(|ran| ran);
    match ran
        .and_then(|output| serde_json::from_str::<Left>(&output).map_err(|error| error.to_string()))
    {
        Ok(left) => {
            ctx.vars = left.vars;
            ctx.req = left.req;
            ctx.logs.extend(left.logs);
            ctx.tests.extend(left.tests);
        }
        Err(error) => ctx.logs.push(ScriptLog {
            level: "error".into(),
            text: format!("script error: {error}"),
        }),
    }
}

/// What an expression over `res`, `req` and `bru` gives; `None` when it gives `undefined`.
pub async fn expression(
    expression: &str,
    res: Option<&ResView>,
    req: &ReqView,
    bru: &BTreeMap<String, String>,
) -> Result<Option<Value>, String> {
    let input = json!({ "expression": expression, "res": res, "req": req, "bru": bru });
    let trust = BruTrust::default();
    let output = tokio::task::spawn_blocking(move || evaluate(input, trust))
        .await
        .map_err(|error| error.to_string())??;
    let answer: Value = serde_json::from_str(&output).map_err(|error| error.to_string())?;
    if let Some(error) = answer.get("error").and_then(Value::as_str) {
        return Err(error.to_owned());
    }
    Ok(answer.get("value").cloned())
}

/// An `assert` row: whether what `expression` gives passes `op` against `expected`, and why not
/// when it fails.
pub async fn assertion(
    expression: &str,
    op: &str,
    expected: &str,
    res: Option<&ResView>,
    req: &ReqView,
    bru: &BTreeMap<String, String>,
) -> Result<(bool, Option<String>), String> {
    let input = json!({ "expression": expression, "op": op, "expected": expected, "res": res, "req": req, "bru": bru });
    let output = tokio::task::spawn_blocking(move || evaluate(input, BruTrust::default()))
        .await
        .map_err(|error| error.to_string())??;
    let answer: Value = serde_json::from_str(&output).map_err(|error| error.to_string())?;
    if let Some(error) = answer
        .get("error")
        .and_then(Value::as_str)
        .filter(|_| answer.get("passed").is_none())
    {
        return Err(error.to_owned());
    }
    let passed = answer
        .get("passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let error = answer
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((passed, error))
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    #[test]
    fn a_script_s_request_cannot_raise_the_run_s_trust() {
        let trust = BruTrust::default();
        let asked = json!({
            "method": "GET",
            "url": "http://127.0.0.1",
            "headers": [],
            "body": { "kind": "none" },
            "skip_tls_verify": true,
            "trust": { "allow_private_network": true, "allow_file_read": true, "allow_insecure_tls": true, "file_root": "/" },
        });
        let request = request_for_script(&asked.to_string(), &trust).unwrap();
        assert_eq!(request.trust, trust);
        assert!(!request.skip_tls_verify);
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            vars: BTreeMap::from([("token".to_owned(), "abc".to_owned())]),
            req: ReqView {
                method: "GET".into(),
                url: "https://example.test".into(),
                headers: BTreeMap::new(),
                body: Value::Null,
                name: "Ping".into(),
            },
            res: Some(ResView {
                status: 200,
                status_text: "OK".into(),
                headers: BTreeMap::new(),
                body: json!({ "id": 7 }),
                response_time: 12,
            }),
            logs: Vec::new(),
            tests: Vec::new(),
            trust: BruTrust::default(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stops_a_script_that_never_ends() {
        let mut ran = ctx();
        let started = std::time::Instant::now();
        run_script("while (true) {}", &mut ran).await;
        assert!(started.elapsed() < TIME_LIMIT + Duration::from_secs(3));
        assert!(
            ran.logs[0].text.starts_with("script error"),
            "{:?}",
            ran.logs
        );
    }
}
