//! The live view of a simulator on the desk. The helper serves the screen as
//! MJPEG over a private local WebSocket; this reads it, keeps the latest frame,
//! and serves that frame to the window over `sim://` when told a new one
//! arrived, so frames never cross IPC and the address never reaches the page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::async_runtime::JoinHandle;
use tauri::http::{header, HeaderName, HeaderValue, Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use super::tools::{choose_device, list_devices, Device};
use super::SimulatorManager;
use crate::error::{AppError, AppResult};

pub const SCHEME: &str = "sim";
pub const FRAME_EVENT: &str = "simulator-frame";
/// The desk shows the screen as JPEG frames, which an image can draw as they come.
const FORMAT: &str = "mjpeg";

#[derive(Default)]
pub struct Views {
    views: Mutex<HashMap<String, View>>,
    /// The device drawn around each simulator's screen, once per device.
    chromes: Mutex<HashMap<String, Arc<Chrome>>>,
}

struct Chrome {
    layout: Value,
    image: Vec<u8>,
    mask: Vec<u8>,
}

struct View {
    viewers: usize,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
    reader: JoinHandle<()>,
}

impl Views {
    /// Draws the device around `udid`'s screen the first time it is asked for. Without it the
    /// screen is shown bare, so a device Xcode has no artwork for still shows.
    async fn chrome(&self, simulators: &SimulatorManager, udid: &str) -> Option<Arc<Chrome>> {
        if let Some(chrome) = self.lock_chromes().get(udid) {
            return Some(Arc::clone(chrome));
        }
        let folder = tempfile::tempdir().ok()?;
        let (image, mask) = (
            folder.path().join("chrome.png"),
            folder.path().join("mask.png"),
        );
        let layout = simulators
            .request(
                "chrome",
                json!({ "udid": udid, "path": image, "mask": mask }),
            )
            .await
            .ok()?;
        let chrome = Arc::new(Chrome {
            layout,
            image: std::fs::read(&image).ok()?,
            mask: std::fs::read(&mask).ok()?,
        });
        self.lock_chromes()
            .insert(udid.to_owned(), Arc::clone(&chrome));
        Some(chrome)
    }

    fn lock_chromes(&self) -> MutexGuard<'_, HashMap<String, Arc<Chrome>>> {
        self.chromes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, View>> {
        self.views
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn latest(&self, udid: &str) -> Option<Vec<u8>> {
        let latest = Arc::clone(&self.lock().get(udid)?.latest);
        let frame = latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        frame
    }
}

#[tauri::command]
pub async fn simulator_view_open(
    app: AppHandle,
    views: State<'_, Views>,
    simulators: State<'_, SimulatorManager>,
    udid: String,
) -> AppResult<Option<Value>> {
    let layout = views
        .chrome(&simulators, &udid)
        .await
        .map(|chrome| chrome.layout.clone());
    if let Some(view) = views.lock().get_mut(&udid) {
        view.viewers += 1;
        return Ok(layout);
    }
    let reply = simulators
        .request("stream", json!({ "udid": udid, "format": FORMAT }))
        .await
        .map_err(AppError::Other)?;
    let (Some(port), Some(token)) = (reply["port"].as_u64(), reply["token"].as_str()) else {
        return Err(AppError::Other("the helper gave no stream address".into()));
    };
    let mut frames = connect(port, token)
        .await
        .map_err(|error| AppError::Other(format!("could not watch the simulator: {error}")))?;
    let latest: Arc<Mutex<Option<Vec<u8>>>> = Arc::default();
    let kept = Arc::clone(&latest);
    let watched = udid.clone();
    let reader = tauri::async_runtime::spawn(async move {
        let mut count = 0u64;
        while let Some(message) = frames.next().await {
            let frame = match message {
                Ok(message) if message.is_binary() => message.into_payload(),
                Ok(_) => continue,
                Err(error) => {
                    let _ = app.emit_to(
                        "main",
                        FRAME_EVENT,
                        json!({ "udid": watched, "error": error.to_string() }),
                    );
                    return;
                }
            };
            *kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(frame.to_vec());
            count += 1;
            let _ = app.emit_to(
                "main",
                FRAME_EVENT,
                json!({ "udid": watched, "frame": count }),
            );
        }
    });
    views.lock().insert(
        udid,
        View {
            viewers: 1,
            latest,
            reader,
        },
    );
    Ok(layout)
}

#[tauri::command]
pub async fn simulator_view_close(
    views: State<'_, Views>,
    simulators: State<'_, SimulatorManager>,
    udid: String,
) -> AppResult<()> {
    let closed = {
        let mut views = views.lock();
        match views.get_mut(&udid) {
            Some(view) if view.viewers > 1 => {
                view.viewers -= 1;
                None
            }
            Some(_) => views.remove(&udid),
            None => None,
        }
    };
    if let Some(view) = closed {
        view.reader.abort();
        let _ = simulators
            .request("stopStream", json!({ "udid": udid, "format": FORMAT }))
            .await;
    }
    Ok(())
}

/// What the person does in the view: the same requests the agent's tools send.
#[derive(Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Input {
    /// One step of a finger moving live; the window sends them one after another.
    Touch {
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
    Button {
        button: String,
    },
    Type {
        text: String,
    },
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[tauri::command]
pub async fn simulator_input(
    simulators: State<'_, SimulatorManager>,
    udid: String,
    input: Input,
) -> AppResult<()> {
    let (kind, mut fields) = match input {
        Input::Touch { phase, x, y } => ("touch", json!({ "phase": phase, "x": x, "y": y })),
        Input::Button { button } => ("button", json!({ "button": button })),
        Input::Type { text } => ("text", json!({ "text": text })),
    };
    if let Value::Object(map) = &mut fields {
        map.retain(|_, value| !value.is_null());
        map.insert("udid".into(), udid.into());
    }
    simulators
        .request(kind, fields)
        .await
        .map(drop)
        .map_err(AppError::Other)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    udid: String,
    name: String,
    os: String,
    booted: bool,
    screen: Option<Value>,
}

impl From<Device> for DeviceSummary {
    fn from(device: Device) -> Self {
        Self {
            screen: device
                .screen
                .map(|(width, height)| json!({ "width": width, "height": height })),
            udid: device.udid,
            name: device.name,
            os: device.os,
            booted: device.booted,
        }
    }
}

/// Every simulator Xcode has, for the person to pick from.
#[tauri::command]
pub async fn simulator_devices(
    simulators: State<'_, SimulatorManager>,
) -> AppResult<Vec<DeviceSummary>> {
    let devices = list_devices(&simulators).await.map_err(AppError::Other)?;
    Ok(devices.into_iter().map(DeviceSummary::from).collect())
}

/// Turns the device the person is looking at, and tells every view of it.
#[tauri::command]
pub async fn simulator_rotate(
    app: AppHandle,
    simulators: State<'_, SimulatorManager>,
    udid: String,
    orientation: String,
) -> AppResult<()> {
    super::tools::rotate(&simulators, &udid, &orientation)
        .await
        .map_err(AppError::Other)?;
    let _ = app.emit_to(
        "main",
        super::tools::ROTATED_EVENT,
        json!({ "udid": udid, "orientation": orientation }),
    );
    Ok(())
}

/// Which way the device is turned, as it reports it.
#[tauri::command]
pub async fn simulator_orientation(
    simulators: State<'_, SimulatorManager>,
    udid: String,
) -> AppResult<String> {
    let reply = simulators
        .request("orientation", json!({ "udid": udid }))
        .await
        .map_err(AppError::Other)?;
    let orientation = reply["orientation"]
        .as_str()
        .unwrap_or("portrait")
        .to_owned();
    simulators.set_orientation(&udid, &orientation);
    Ok(orientation)
}

#[tauri::command]
pub fn simulator_set_enabled(enabled: bool) {
    super::set_enabled(enabled);
}

/// What Settings shows about the simulator: the Xcode in use, its iOS runtimes,
/// and where the helper stands.
#[tauri::command]
pub async fn simulator_setup(app: AppHandle) -> AppResult<Value> {
    let output = |program: &str, args: &[&str]| {
        sikemux_process::user_environment::command(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let runtimes: Vec<String> = output("xcrun", &["simctl", "list", "runtimes", "--json"])
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|list| {
            list["runtimes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|runtime| runtime["isAvailable"].as_bool() == Some(true))
                .filter_map(|runtime| runtime["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "xcode": output("xcode-select", &["-p"]),
        "runtimes": runtimes,
        "helper": if crate::sim::local_helper().is_some() {
            "built with this copy of Sikemux"
        } else if crate::sim::installed(&app) {
            "ready"
        } else if crate::sim::published_helper().is_some() {
            "downloaded when first needed"
        } else {
            "not included in this build"
        },
    }))
}

/// Whether this Mac can run simulators, so the window offers them only where they work.
#[tauri::command]
pub fn simulator_available() -> bool {
    super::capable()
}

/// The device to show when the person opens the simulator: the one the agent is
/// using, else a booted iPhone, else an iPhone on the newest iOS.
#[tauri::command]
pub async fn simulator_preferred(
    simulators: State<'_, SimulatorManager>,
    agent_id: String,
) -> AppResult<DeviceSummary> {
    if let Some(device) = simulators.attached(&agent_id) {
        return Ok(device.into());
    }
    let devices = list_devices(&simulators).await.map_err(AppError::Other)?;
    let device = choose_device(&devices, None).map_err(AppError::Other)?;
    Ok(device.clone().into())
}

/// Boots the device the person picked and makes it the agent's, so the person
/// and the agent keep looking at the same screen.
#[tauri::command]
pub async fn simulator_attach(
    simulators: State<'_, SimulatorManager>,
    agent_id: String,
    udid: String,
) -> AppResult<DeviceSummary> {
    let device = list_devices(&simulators)
        .await
        .map_err(AppError::Other)?
        .into_iter()
        .find(|device| device.udid == udid)
        .ok_or_else(|| AppError::Other(format!("no simulator {udid}")))?;
    simulators
        .request("boot", json!({ "udid": udid }))
        .await
        .map_err(AppError::Other)?;
    simulators.attach(&agent_id, device.clone());
    Ok(DeviceSummary {
        booted: true,
        ..device.into()
    })
}

/// Saves the screen at full size to the Desktop as `name`, the way Simulator.app does.
#[tauri::command]
pub async fn simulator_save_screenshot(
    app: AppHandle,
    simulators: State<'_, SimulatorManager>,
    udid: String,
    name: String,
) -> AppResult<String> {
    let plain = std::path::Path::new(&name)
        .file_name()
        .is_some_and(|file| file == name.as_str());
    if !plain || !name.ends_with(".png") {
        return Err(AppError::BadArg(
            "a screenshot name is a plain .png file name",
        ));
    }
    let desktop = app
        .path()
        .desktop_dir()
        .map_err(|error| AppError::Other(format!("no Desktop folder: {error}")))?;
    let path = desktop.join(&name);
    simulators
        .request("screenshot", json!({ "udid": udid, "path": path }))
        .await
        .map_err(AppError::Other)?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn simulator_shutdown(
    simulators: State<'_, SimulatorManager>,
    udid: String,
) -> AppResult<()> {
    simulators
        .request("shutdown", json!({ "udid": udid }))
        .await
        .map(drop)
        .map_err(AppError::Other)
}

/// Opens the helper's stream, which admits a viewer that offers its token as the WebSocket subprotocol.
async fn connect(
    port: u64,
    token: &str,
) -> Result<
    tokio_websockets::WebSocketStream<tokio_websockets::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let token = HeaderValue::from_str(token).map_err(|error| error.to_string())?;
    let (stream, _) = tokio_websockets::ClientBuilder::new()
        .uri(&format!("ws://127.0.0.1:{port}"))
        .map_err(|error| error.to_string())?
        .add_header(HeaderName::from_static("sec-websocket-protocol"), token)
        .map_err(|error| error.to_string())?
        .connect()
        .await
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

/// `sim://localhost/<udid>/<frame>` answers with that simulator's latest frame;
/// the frame number only keeps the window from reusing a cached one.
/// `<udid>/chrome` and `<udid>/mask` are the device drawn around the screen and
/// the shape of the screen.
pub fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    if context.webview_label() != "main" {
        responder.respond(status(StatusCode::FORBIDDEN));
        return;
    }
    let mut parts = request.uri().path().trim_start_matches('/').split('/');
    let udid = parts.next().unwrap_or_default().to_owned();
    let views = context.app_handle().state::<Views>();
    let chrome = || views.lock_chromes().get(&udid).cloned();
    let (body, kind) = match parts.next() {
        Some("chrome") => (chrome().map(|chrome| chrome.image.clone()), "image/png"),
        Some("mask") => (chrome().map(|chrome| chrome.mask.clone()), "image/png"),
        _ => (views.latest(&udid), "image/jpeg"),
    };
    responder.respond(match body {
        Some(image) => Response::builder()
            .header(header::CONTENT_TYPE, kind)
            .header(header::CACHE_CONTROL, "no-store")
            // The window loads the screen's shape as a CSS mask, which a browser fetches only
            // across origins the answer allows; without this the masked screen draws nothing.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(image)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR)),
        None => status(StatusCode::NOT_FOUND),
    });
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .unwrap_or_default()
}
