//! The iOS Simulator. Devices are driven by the `sikemux-sim` helper, which
//! talks to Apple's CoreSimulator through facebook/idb's FBSimulatorControl.
//! This module starts the helper, sends it one JSON request per line and hands
//! each answer back to the caller that asked, matched by id.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use crate::error::{AppError, AppResult};
use crate::voice_models::{self, ModelFile};

pub const SIM_EVENT: &str = "sim";
/// Tells the page a screen it was watching stopped, and why, unless it asked for that itself.
pub const WATCH_ENDED_EVENT: &str = "simulator-watch-ended";

/// Booting a device the first time can take most of a minute.
const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// A request given at least this long that still goes unanswered counts towards restarting the helper.
const HANG: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_secs(10)
};
const HANGS_BEFORE_RESTART: u32 = 3;
/// How many frames may wait for the page before the helper's socket is left
/// unread, which is how the helper learns to drop frames.
const UNREAD_FRAMES: u64 = 12;

type Reply = Result<Value, AppError>;
type Waiting = Arc<Mutex<Pending>>;

/// The requests one helper process still owes an answer, closed once it stops.
#[derive(Default)]
struct Pending {
    closed: bool,
    senders: HashMap<u64, oneshot::Sender<Reply>>,
}

struct Helper {
    child: Child,
    stdin: ChildStdin,
    waiting: Waiting,
    generation: u64,
    hangs: u32,
}

struct Watch {
    task: tauri::async_runtime::JoinHandle<()>,
    udid: String,
    format: String,
    read: Arc<Progress>,
}

/// How many frames of a watch the page has read, and a wake-up for each report.
#[derive(Default)]
struct Progress {
    frames: AtomicU64,
    reported: tokio::sync::Notify,
}

impl Progress {
    fn report(&self, frames: u64) {
        self.frames.fetch_max(frames, Ordering::SeqCst);
        self.reported.notify_waiters();
    }

    async fn caught_up(&self, sent: u64) {
        loop {
            let reported = self.reported.notified();
            if sent.saturating_sub(self.frames.load(Ordering::SeqCst)) < UNREAD_FRAMES {
                return;
            }
            reported.await;
        }
    }
}

#[derive(Clone, Default)]
pub struct SimManager {
    helper: Arc<Mutex<Option<Helper>>>,
    next_id: Arc<AtomicU64>,
    generation: Arc<AtomicU64>,
    watches: Arc<Mutex<HashMap<u64, Watch>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimStatus {
    supported: bool,
    installed: bool,
    reason: Option<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl SimManager {
    pub fn drain(&self) {
        self.drop_watches();
        if let Some(mut helper) = lock(&self.helper).take() {
            let _ = helper.child.kill();
            let _ = helper.child.wait();
            fail_all(&helper.waiting, "Sikemux is quitting");
        }
    }

    /// Stops every screen stream, for a page that reloaded and no longer reads them.
    pub fn drop_watches(&self) {
        for (_, watch) in lock(&self.watches).drain() {
            watch.task.abort();
        }
    }

    /// Sends one request, such as `{"type": "tap", "x": 10, "y": 20}`, and waits for its answer.
    pub async fn call(&self, executable: PathBuf, request: Map<String, Value>) -> AppResult<Value> {
        let timeout = if request.get("type").and_then(Value::as_str) == Some("boot") {
            BOOT_TIMEOUT
        } else {
            REQUEST_TIMEOUT
        };
        self.call_within(executable, request, timeout).await
    }

    pub async fn call_within(
        &self,
        executable: PathBuf,
        mut request: Map<String, Value>,
        timeout: Duration,
    ) -> AppResult<Value> {
        let kind = request
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        request.insert("id".into(), Value::from(id));
        let (sender, receiver) = oneshot::channel();
        let (waiting, generation) = self.send(executable, id, sender, &Value::Object(request))?;
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(reply)) => {
                self.note_answer(generation);
                reply
            }
            Ok(Err(_)) => Err(AppError::Other("the simulator helper stopped".into())),
            Err(_) => {
                lock(&waiting).senders.remove(&id);
                if timeout >= HANG {
                    self.note_hang(generation);
                }
                Err(AppError::Other(format!(
                    "the simulator did not answer `{kind}` in time"
                )))
            }
        }
    }

    fn send(
        &self,
        executable: PathBuf,
        id: u64,
        sender: oneshot::Sender<Reply>,
        request: &Value,
    ) -> AppResult<(Waiting, u64)> {
        let mut slot = lock(&self.helper);
        let running = match slot.as_mut() {
            Some(helper) => matches!(helper.child.try_wait(), Ok(None)),
            None => false,
        };
        if !running {
            *slot = Some(self.spawn(executable.clone())?);
        }
        let mut sender = Some(sender);
        for _ in 0..2 {
            let helper = slot.as_mut().expect("helper was just started");
            let mut pending = lock(&helper.waiting);
            if !pending.closed {
                pending
                    .senders
                    .insert(id, sender.take().expect("sent once"));
                break;
            }
            drop(pending);
            *slot = Some(self.spawn(executable.clone())?);
        }
        let helper = slot.as_mut().expect("helper was just started");
        if sender.is_some() {
            return Err(AppError::Other("the simulator helper stopped".into()));
        }
        let mut line = request.to_string();
        line.push('\n');
        let written = helper
            .stdin
            .write_all(line.as_bytes())
            .and_then(|()| helper.stdin.flush());
        if let Err(error) = written {
            lock(&helper.waiting).senders.remove(&id);
            return Err(AppError::Other(format!(
                "simulator helper stopped listening: {error}"
            )));
        }
        Ok((Arc::clone(&helper.waiting), helper.generation))
    }

    fn note_answer(&self, generation: u64) {
        if let Some(helper) = lock(&self.helper).as_mut() {
            if helper.generation == generation {
                helper.hangs = 0;
            }
        }
    }

    /// A helper that stops answering is killed, so the next request starts a fresh one.
    fn note_hang(&self, generation: u64) {
        let mut slot = lock(&self.helper);
        let Some(helper) = slot
            .as_mut()
            .filter(|helper| helper.generation == generation)
        else {
            return;
        };
        helper.hangs += 1;
        if helper.hangs < HANGS_BEFORE_RESTART {
            return;
        }
        if let Some(mut helper) = slot.take() {
            let _ = helper.child.kill();
            let _ = helper.child.wait();
            fail_all(
                &helper.waiting,
                "the simulator helper stopped answering, so it was restarted; try again",
            );
        }
    }

    fn spawn(&self, executable: PathBuf) -> AppResult<Helper> {
        let mut child = sikemux_process::user_environment::command(executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                AppError::Other(format!("could not start the simulator helper: {error}"))
            })?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let waiting: Waiting = Arc::default();
        let answers = Arc::clone(&waiting);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some((id, reply)) = parse_reply(&line) {
                    if let Some(sender) = lock(&answers).senders.remove(&id) {
                        let _ = sender.send(reply);
                    }
                }
            }
            fail_all(&answers, "the simulator helper stopped");
        });
        Ok(Helper {
            child,
            stdin,
            waiting,
            generation: self.generation.fetch_add(1, Ordering::SeqCst) + 1,
            hangs: 0,
        })
    }

    fn end_watch(&self, id: u64) -> Option<Watch> {
        lock(&self.watches).remove(&id)
    }

    /// Whether another watch still streams this device in this format, so its stream must keep running.
    fn streamed(&self, udid: &str, format: &str) -> bool {
        lock(&self.watches)
            .values()
            .any(|watch| watch.udid == udid && watch.format == format)
    }
}

fn fail_all(waiting: &Waiting, message: &str) {
    let mut pending = lock(waiting);
    pending.closed = true;
    for (_, sender) in pending.senders.drain() {
        let _ = sender.send(Err(AppError::Other(message.into())));
    }
}

/// An answer from the helper: its request id, and either the result fields or the error it reported.
fn parse_reply(line: &str) -> Option<(u64, Reply)> {
    let Value::Object(mut fields) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    let id = fields.remove("id")?.as_u64()?;
    match fields.remove("type")?.as_str()? {
        "result" => Some((id, Ok(Value::Object(fields)))),
        "error" => {
            let message = fields
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the simulator failed");
            let message = match fields.get("reason").and_then(Value::as_str) {
                Some(reason @ "streamUnavailable") => format!("{message} ({reason})"),
                _ => message.to_owned(),
            };
            Some((id, Err(AppError::Other(message))))
        }
        _ => None,
    }
}

/// A helper built alongside the app, as `make dev` does.
pub(crate) fn local_helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SIKEMUX_SIM_EXECUTABLE") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join("sikemux-sim");
    beside.is_file().then_some(beside)
}

/// The helper published beside this release, which the app downloads the first time it is needed.
pub(crate) fn published_helper() -> Option<ModelFile> {
    Some(ModelFile {
        path: option_env!("SIKEMUX_SIM_HELPER_ASSET")?.into(),
        size: option_env!("SIKEMUX_SIM_HELPER_SIZE")?.parse().ok()?,
        sha256: option_env!("SIKEMUX_SIM_HELPER_SHA256")?.into(),
    })
}

fn downloaded_helper(app: &AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("simulator data directory unavailable: {error}")))?
        .join("sim")
        .join("sikemux-sim"))
}

fn matches(path: &Path, size: u64, sha256: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() == size)
        && voice_models::hash_file(path).is_ok_and(|hash| hash == sha256)
}

/// The downloaded helper once its hash was checked. Holding the lock also keeps
/// a second caller from downloading it again while the first one does.
fn verified() -> &'static tokio::sync::Mutex<Option<PathBuf>> {
    static VERIFIED: tokio::sync::Mutex<Option<PathBuf>> = tokio::sync::Mutex::const_new(None);
    &VERIFIED
}

pub(crate) fn installed(app: &AppHandle) -> bool {
    if local_helper().is_some() {
        return true;
    }
    if let Ok(known) = verified().try_lock() {
        if known.is_some() {
            return true;
        }
    }
    published_helper()
        .zip(downloaded_helper(app).ok())
        .is_some_and(|(helper, path)| {
            std::fs::metadata(path).is_ok_and(|meta| meta.len() == helper.size)
        })
}

/// The helper to run, downloading the published one first if this build has none beside it.
pub(crate) async fn executable(app: &AppHandle) -> AppResult<PathBuf> {
    if let Some(local) = local_helper() {
        return Ok(local);
    }
    let mut known = verified().lock().await;
    if let Some(path) = known.as_ref() {
        return Ok(path.clone());
    }
    let helper = published_helper().ok_or_else(|| {
        AppError::Other("This build does not include the simulator helper.".into())
    })?;
    let destination = downloaded_helper(app)?;
    let (check, size, sha256) = (destination.clone(), helper.size, helper.sha256.clone());
    let present = tauri::async_runtime::spawn_blocking(move || matches(&check, size, &sha256))
        .await
        .unwrap_or(false);
    if !present {
        let url = format!(
            "https://github.com/nodelike/sikemux/releases/download/v{}/{}",
            env!("CARGO_PKG_VERSION"),
            helper.path
        );
        let mut reported = 0.0;
        voice_models::download_executable(&url, &destination, &helper, |bytes| {
            let fraction = bytes as f64 / helper.size as f64;
            if fraction - reported >= 0.01 || fraction >= 1.0 {
                reported = fraction;
                let _ = app.emit_to(
                    "main",
                    SIM_EVENT,
                    json!({ "type": "progress", "fraction": fraction }),
                );
            }
        })
        .await?;
    }
    *known = Some(destination.clone());
    Ok(destination)
}

/// Why this Mac cannot run the simulator, or nothing when it can.
pub(crate) fn unsupported_reason() -> Option<String> {
    crate::simulator::unsupported_reason()
}

#[tauri::command]
pub async fn sim_status(app: AppHandle) -> AppResult<SimStatus> {
    let reason = tauri::async_runtime::spawn_blocking(unsupported_reason)
        .await
        .unwrap_or_else(|error| Some(error.to_string()));
    Ok(SimStatus {
        supported: reason.is_none(),
        installed: reason.is_none() && installed(&app),
        reason,
    })
}

/// Downloads the helper if this build needs to, reporting progress as `sim` events.
#[tauri::command]
pub async fn sim_prepare(app: AppHandle) -> AppResult<()> {
    supported().await?;
    executable(&app).await.map(|_| ())
}

async fn supported() -> AppResult<()> {
    match tauri::async_runtime::spawn_blocking(unsupported_reason).await {
        Ok(None) => Ok(()),
        Ok(Some(reason)) => Err(AppError::Other(reason)),
        Err(error) => Err(AppError::Other(error.to_string())),
    }
}

#[tauri::command]
pub async fn sim_call(
    app: AppHandle,
    request: Map<String, Value>,
    sim: State<'_, SimManager>,
) -> AppResult<Value> {
    supported().await?;
    let boot = request.get("type").and_then(Value::as_str) == Some("boot");
    let udid = request
        .get("udid")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let answer = sim.call(executable(&app).await?, request).await?;
    if let (true, Some(udid)) = (boot, udid) {
        app.state::<crate::simulator::SimulatorManager>()
            .note_booted(&udid);
    }
    Ok(answer)
}

/// Why forwarding stopped: the page went away, or the helper's stream ended.
#[derive(Debug, PartialEq)]
enum Ended {
    PageGone,
    Stream(String),
}

/// Reads the helper's length-prefixed frames from 127.0.0.1 and hands each to
/// the page as raw bytes. While the page is [`UNREAD_FRAMES`] behind, the
/// socket is left unread, so the helper drops frames rather than the page
/// falling further behind.
async fn forward_frames(
    port: u16,
    token: String,
    read: Arc<Progress>,
    on_frame: Channel<Response>,
) -> Ended {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let stream = async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        socket.set_nodelay(true)?;
        socket.write_all(format!("{token}\n").as_bytes()).await?;
        Ok::<_, std::io::Error>(socket)
    };
    let mut socket = match stream.await {
        Ok(socket) => socket,
        Err(error) => return Ended::Stream(error.to_string()),
    };
    let mut sent = 0u64;
    loop {
        read.caught_up(sent).await;
        let frame = async {
            let length = socket.read_u32().await? as usize;
            let mut frame = vec![0; length];
            socket.read_exact(&mut frame).await?;
            Ok::<_, std::io::Error>(frame)
        };
        let frame = match frame.await {
            Ok(frame) => frame,
            Err(error) => return Ended::Stream(error.to_string()),
        };
        if on_frame.send(Response::new(frame)).is_err() {
            return Ended::PageGone;
        }
        sent += 1;
    }
}

/// Streams a device's screen to `on_frame`, and returns an id `sim_unwatch`
/// stops it by. The page tells how many frames it has read with
/// `sim_watch_read`; a watch that ends on its own sends `simulator-watch-ended`.
#[tauri::command]
pub async fn sim_watch(
    app: AppHandle,
    udid: String,
    format: String,
    on_frame: Channel<Response>,
    sim: State<'_, SimManager>,
) -> AppResult<u64> {
    supported().await?;
    let request = json!({ "type": "stream", "udid": udid, "format": format });
    let stream = sim
        .call(
            executable(&app).await?,
            request.as_object().cloned().unwrap_or_default(),
        )
        .await?;
    let port = stream["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| AppError::Other("the simulator helper gave no stream port".into()))?;
    let token = stream["token"].as_str().unwrap_or_default().to_owned();
    let id = sim.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let read = Arc::new(Progress::default());
    let mut watches = lock(&sim.watches);
    let manager = sim.inner().clone();
    let task = tauri::async_runtime::spawn({
        let read = Arc::clone(&read);
        async move {
            let ended = forward_frames(port, token, read, on_frame).await;
            if manager.end_watch(id).is_none() {
                return;
            }
            let reason = match ended {
                Ended::PageGone => return,
                Ended::Stream(error) => format!("the stream stopped: {error}"),
            };
            let _ = app.emit_to(
                "main",
                WATCH_ENDED_EVENT,
                json!({ "id": id, "reason": reason }),
            );
        }
    });
    watches.insert(
        id,
        Watch {
            task,
            udid,
            format,
            read,
        },
    );
    Ok(id)
}

/// The page has read this many frames of a watch.
#[tauri::command]
pub fn sim_watch_read(id: u64, frames: u64, sim: State<'_, SimManager>) {
    if let Some(watch) = lock(&sim.watches).get(&id) {
        watch.read.report(frames);
    }
}

#[tauri::command]
pub async fn sim_unwatch(app: AppHandle, id: u64, sim: State<'_, SimManager>) -> AppResult<()> {
    let Some(watch) = sim.end_watch(id) else {
        return Ok(());
    };
    watch.task.abort();
    if !sim.streamed(&watch.udid, &watch.format) {
        let request = json!({ "type": "stopStream", "udid": watch.udid, "format": watch.format });
        let _ = sim
            .call(
                executable(&app).await?,
                request.as_object().cloned().unwrap_or_default(),
            )
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_goes_to_the_request_it_answers() {
        let (id, reply) = parse_reply(r#"{"id":7,"type":"result","pid":42}"#).unwrap();
        assert_eq!(id, 7);
        assert_eq!(reply.unwrap(), serde_json::json!({ "pid": 42 }));
    }

    #[test]
    fn an_error_carries_the_helpers_message() {
        let (id, reply) = parse_reply(r#"{"id":3,"type":"error","reason":"notBooted","message":"iPhone 17 is not running. Boot it first."}"#).unwrap();
        assert_eq!(id, 3);
        assert_eq!(
            reply.unwrap_err().to_string(),
            "iPhone 17 is not running. Boot it first."
        );
    }

    fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sikemux-sim");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    fn request(kind: &str) -> Map<String, Value> {
        json!({ "type": kind }).as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn a_helper_that_stops_fails_only_its_own_requests() {
        let (_dir, path) = script(
            "#!/bin/sh\nwhile IFS= read -r line; do\n  id=$(printf '%s' \"$line\" | sed -E 's/.*\"id\":([0-9]+).*/\\1/')\n  case \"$line\" in\n    *quit*) exit 0 ;;\n    *) echo \"{\\\"id\\\":$id,\\\"type\\\":\\\"result\\\"}\" ;;\n  esac\ndone\n",
        );
        let sim = SimManager::default();
        assert!(sim.call(path.clone(), request("quit")).await.is_err());
        let first = sim.generation.load(Ordering::SeqCst);
        sim.call(path.clone(), request("devices"))
            .await
            .expect("a new helper answers");
        assert_eq!(sim.generation.load(Ordering::SeqCst), first + 1);
        sim.drain();
    }

    #[tokio::test]
    async fn a_helper_that_stops_answering_is_restarted() {
        let (_dir, path) = script("#!/bin/sh\nwhile IFS= read -r line; do :; done\n");
        let sim = SimManager::default();
        for _ in 0..HANGS_BEFORE_RESTART {
            let _ = sim.call_within(path.clone(), request("tree"), HANG).await;
        }
        let first = sim.generation.load(Ordering::SeqCst);
        let _ = sim
            .call_within(path.clone(), request("tree"), Duration::from_millis(10))
            .await;
        assert_eq!(
            sim.generation.load(Ordering::SeqCst),
            first + 1,
            "a fresh helper was started"
        );
        sim.drain();
    }

    /// Drives a real simulator: `SIKEMUX_SIM_EXECUTABLE=… cargo test sim -- --ignored`.
    #[tokio::test]
    #[ignore = "needs Xcode, a simulator and a built sikemux-sim"]
    async fn the_helper_lists_boots_and_reads_a_device() {
        let sim = SimManager::default();
        let helper = local_helper().expect("set SIKEMUX_SIM_EXECUTABLE to a built sikemux-sim");
        let call = |value: Value| sim.call(helper.clone(), value.as_object().cloned().unwrap());
        let devices = call(json!({ "type": "devices" })).await.unwrap();
        let device = devices["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|device| device["state"] == "booted")
            .or_else(|| devices["devices"].get(0))
            .unwrap()
            .clone();
        let udid = device["udid"].as_str().unwrap().to_owned();
        let was_booted = device["state"] == "booted";
        call(json!({ "type": "boot", "udid": udid })).await.unwrap();
        let tree = call(json!({ "type": "tree", "udid": udid })).await.unwrap();
        assert!(tree["elements"].is_array());
        let missing =
            call(json!({ "type": "tapLabel", "udid": udid, "label": "no such label anywhere" }))
                .await
                .unwrap_err();
        assert!(
            missing.to_string().contains("no such label anywhere"),
            "{missing}"
        );
        if !was_booted {
            call(json!({ "type": "shutdown", "udid": udid }))
                .await
                .unwrap();
        }
        sim.drain();
    }

    async fn helper_socket(frames: Vec<&'static [u8]>) -> (u16, tokio::task::JoinHandle<[u8; 6]>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let helper = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut token = [0u8; 6];
            socket.read_exact(&mut token).await.unwrap();
            for frame in frames {
                socket.write_u32(frame.len() as u32).await.unwrap();
                socket.write_all(frame).await.unwrap();
            }
            token
        });
        (port, helper)
    }

    type Received = Arc<Mutex<Vec<Vec<u8>>>>;

    fn recording_channel() -> (Channel<Response>, Received) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Raw(bytes) = body {
                sink.lock().unwrap().push(bytes);
            }
            Ok(())
        });
        (channel, received)
    }

    #[tokio::test]
    async fn frames_from_the_helpers_socket_reach_the_page_as_raw_bytes() {
        let (port, helper) = helper_socket(vec![b"first", b"second"]).await;
        let (channel, received) = recording_channel();

        let ended = forward_frames(port, "token".into(), Arc::default(), channel).await;

        assert_eq!(&helper.await.unwrap(), b"token\n");
        assert_eq!(
            *received.lock().unwrap(),
            vec![b"first".to_vec(), b"second".to_vec()]
        );
        assert!(
            matches!(ended, Ended::Stream(_)),
            "the stream ends when the helper closes its socket"
        );
    }

    #[tokio::test]
    async fn the_socket_waits_while_the_page_is_behind() {
        let frames: Vec<&'static [u8]> = vec![b"frame"; UNREAD_FRAMES as usize + 5];
        let (port, _helper) = helper_socket(frames).await;
        let (channel, received) = recording_channel();
        let read = Arc::new(Progress::default());
        let forwarding = tokio::spawn(forward_frames(
            port,
            "token".into(),
            Arc::clone(&read),
            channel,
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(received.lock().unwrap().len(), UNREAD_FRAMES as usize);
        read.report(UNREAD_FRAMES);
        let ended = forwarding.await.unwrap();
        assert!(matches!(ended, Ended::Stream(_)));
        assert_eq!(
            received.lock().unwrap().len(),
            UNREAD_FRAMES as usize + 5,
            "reading on once the page catches up"
        );
    }

    #[test]
    fn a_stream_the_helper_cannot_encode_says_so_by_name() {
        let (_, reply) = parse_reply(
            r#"{"id":4,"type":"error","reason":"streamUnavailable","message":"No H.264 encoder."}"#,
        )
        .unwrap();
        assert!(reply.unwrap_err().to_string().contains("streamUnavailable"));
    }

    #[test]
    fn lines_that_answer_no_request_are_ignored() {
        assert!(parse_reply(
            r#"{"type":"error","reason":"protocol","message":"Could not read the request"}"#
        )
        .is_none());
        assert!(parse_reply("not json").is_none());
        assert!(parse_reply(r#"{"id":1,"type":"progress"}"#).is_none());
    }
}
