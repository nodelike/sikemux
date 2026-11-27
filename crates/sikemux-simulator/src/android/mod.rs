//! Android emulators and phones, driven through the SDK's `adb` and a small
//! program Sikemux runs on the device itself. It answers the same requests the
//! iOS helper does, in the same shapes and in device points (Android's dp), so the
//! agents' `sim` tool and the desk drive both alike.

mod grpc;
pub mod install;
mod logcat;
pub mod pair;
pub(crate) mod sdk;

use sikemux_process::command::Stdio;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;

use crate::helper::{Program, SimManager};
use crate::SimResult;
use logcat::Logcat;

const DEVICE_PROGRAM: &[u8] = include_bytes!("../../../../helpers/android/sikemux-device.jar");
const DEVICE_JAR: &str = "/data/local/tmp/sikemux-device.jar";
const VIRTUAL: &str = "avd:";
const PLUGGED: &str = "adb:";
const BOOT_POLL: Duration = Duration::from_secs(1);
const SERIAL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const SERIAL_READY_STEP: Duration = Duration::from_millis(250);
const AVD_READY_WAIT: Duration = Duration::from_secs(6);
const PHONE_READY_WAIT: Duration = Duration::from_secs(3);
const PACKAGE_SETTLE_TRIES: usize = 8;
const PACKAGE_SETTLE_STEP: Duration = Duration::from_millis(150);
const LOW_DATA_MB: u64 = 500;
const LOW_DATA_KB: u64 = LOW_DATA_MB * 1024;
const MAX_REPORTED_PACKAGES: usize = 5;
const STORAGE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const PACKAGE_SIZES_TIMEOUT: Duration = Duration::from_secs(10);
const PACKAGE_SIZES_CMD: &str = r#"pm list packages -3 -f | while IFS= read -r line; do p=${line#package:}; [ "$p" = "$line" ] && continue; pkg=${p##*=}; apk=${p%=*}; dir=${apk%/*}; set -- $(du -sk "$dir" 2>/dev/null); kb=$1; if [ -z "$kb" ]; then set -- $(du -sk "$apk" 2>/dev/null); kb=$1; fi; printf '%s %s\n' "${kb:-0}" "$pkg"; done"#;
const SWIPE_STEP: f64 = 1.0 / 60.0;

/// Whether `udid` names an Android device rather than an iOS simulator.
pub fn is_android(udid: &str) -> bool {
    udid.starts_with(VIRTUAL) || udid.starts_with(PLUGGED)
}

/// Whether `udid` names a phone plugged in over adb, whose screen streams as H.264, rather than an emulator.
pub fn is_phone(udid: &str) -> bool {
    udid.starts_with(PLUGGED)
}

/// A device's screen as it sits upright, and how many pixels make a point.
#[derive(Clone, Copy, Debug)]
struct Display {
    width: f64,
    height: f64,
    scale: f64,
}

#[derive(Default)]
pub struct Android {
    data_dir: Option<PathBuf>,
    #[cfg(all(test, unix))]
    sdk_override: Option<PathBuf>,
    sessions: Mutex<HashMap<String, SimManager>>,
    displays: Mutex<HashMap<String, Display>>,
    launchers: Mutex<HashMap<String, Option<String>>>,
    rotations: Mutex<HashMap<String, u8>>,
    pids: Mutex<HashMap<(String, String), i64>>,
    logs: Mutex<HashMap<String, Arc<Logcat>>>,
    releases: Mutex<HashMap<String, String>>,
}

/// What a request is about: a device that may be off, or one adb can reach now.
struct Target {
    serial: Option<String>,
    avd: Option<String>,
    grpc: Option<(u16, String)>,
}

impl Android {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        Self {
            data_dir,
            ..Self::default()
        }
    }

    #[cfg(all(test, unix))]
    fn with_sdk(sdk_root: PathBuf) -> Self {
        Self {
            sdk_override: Some(sdk_root),
            ..Self::default()
        }
    }

    pub fn sdk(&self) -> Option<PathBuf> {
        #[cfg(all(test, unix))]
        if let Some(root) = &self.sdk_override {
            return Some(root.clone());
        }
        sdk::root(self.data_dir.as_deref())
    }

    fn adb_path(&self) -> SimResult<PathBuf> {
        let root = self.sdk().ok_or(NO_SDK)?;
        let adb = sdk::adb(&root);
        adb.is_file().then_some(adb).ok_or_else(|| {
            format!(
                "the Android SDK at {} has no platform-tools",
                root.display()
            )
        })
    }

    async fn adb(&self, args: &[&str], limit: Duration) -> SimResult<String> {
        let adb = self.adb_path()?;
        let mut command = sikemux_process::command::Command::into_async(
            sikemux_process::user_environment::command(adb),
        );
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(limit, command.output())
            .await
            .map_err(|_| format!("adb {} did not answer in time", args.join(" ")))?
            .map_err(|error| format!("could not run adb: {error}"))?;
        let text = String::from_utf8_lossy(&output.stdout).replace('\r', "");
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(if error.is_empty() {
                text.trim().to_owned()
            } else {
                error
            });
        }
        Ok(text)
    }

    async fn shell(&self, serial: &str, command: &str) -> SimResult<String> {
        self.adb(&["-s", serial, "shell", command], Duration::from_secs(30))
            .await
    }

    /// The devices adb can see: serial, state and model.
    async fn plugged(&self) -> Vec<(String, String, String)> {
        let Ok(listed) = self.adb(&["devices", "-l"], Duration::from_secs(15)).await else {
            return Vec::new();
        };
        listed
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let serial = words.next()?.to_owned();
                let state = words.next()?.to_owned();
                let model = words
                    .find_map(|word| word.strip_prefix("model:"))
                    .map(|model| model.replace('_', " "))
                    .unwrap_or_else(|| serial.clone());
                Some((serial, state, model))
            })
            .collect()
    }

    async fn release(&self, serial: &str) -> String {
        if let Some(known) = self.releases.locked().get(serial) {
            return known.clone();
        }
        let release = self
            .shell(serial, "getprop ro.build.version.release")
            .await
            .map(|text| text.trim().to_owned())
            .unwrap_or_default();
        let os = format!("Android {release}").trim().to_owned();
        self.releases.locked().insert(serial.to_owned(), os.clone());
        os
    }

    /// Every Android device: the virtual ones, on or off, then phones adb reaches.
    pub async fn devices(&self) -> Vec<Value> {
        if self.sdk().is_none() {
            return Vec::new();
        }
        let running = tokio::task::spawn_blocking(sdk::running)
            .await
            .unwrap_or_default();
        let avds = tokio::task::spawn_blocking(sdk::avds)
            .await
            .unwrap_or_default();
        let plugged = self.plugged().await;
        let mut devices = Vec::new();
        for avd in avds {
            let live = running.iter().find(|running| running.avd == avd.id);
            let booted = live.is_some_and(|live| {
                plugged
                    .iter()
                    .any(|(serial, state, _)| *serial == live.serial && state == "device")
            });
            devices.push(json!({
                "udid": format!("{VIRTUAL}{}", avd.id),
                "name": avd.name,
                "runtime": avd.api.as_deref().map_or("Android".into(), android_version),
                "state": if booted { "booted" } else { "shutdown" },
                "platform": "android",
            }));
        }
        for (serial, state, model) in plugged {
            if serial.starts_with("emulator-") {
                continue;
            }
            let runtime = match state.as_str() {
                "device" => self.release(&serial).await,
                _ => "Android".into(),
            };
            devices.push(json!({
                "udid": format!("{PLUGGED}{serial}"),
                "name": model,
                "runtime": runtime,
                "state": if state == "device" { "booted" } else { state.as_str() },
                "platform": "android",
            }));
        }
        devices
    }

    fn target(&self, udid: &str) -> Target {
        if let Some(serial) = udid.strip_prefix(PLUGGED) {
            return Target {
                serial: Some(serial.to_owned()),
                avd: None,
                grpc: None,
            };
        }
        let avd = udid.strip_prefix(VIRTUAL).unwrap_or(udid).to_owned();
        let running = sdk::running()
            .into_iter()
            .find(|running| running.avd == avd);
        Target {
            serial: running.as_ref().map(|running| running.serial.clone()),
            grpc: running.and_then(|running| running.grpc),
            avd: Some(avd),
        }
    }

    async fn serial(&self, udid: &str) -> SimResult<String> {
        let target = self.target(udid);
        let is_avd = target.avd.is_some();
        let label = target.avd.as_deref().unwrap_or(udid);
        let serial = target.serial.ok_or_else(|| {
            format!("{label} is not running. Call sim attach to start it, or ask the person.")
        })?;
        self.wait_for_serial_ready(serial, is_avd, || self.target(udid).serial, label)
            .await
    }

    async fn wait_for_serial_ready(
        &self,
        mut serial: String,
        is_avd: bool,
        mut refresh: impl FnMut() -> Option<String>,
        missing_label: &str,
    ) -> SimResult<String> {
        let wait = if is_avd {
            AVD_READY_WAIT
        } else {
            PHONE_READY_WAIT
        };
        let mut deadline = None;
        loop {
            let state = classify_device_state(
                self.adb(&["-s", &serial, "get-state"], SERIAL_PROBE_TIMEOUT)
                    .await,
            );
            match state {
                DeviceState::Ready => return Ok(serial),
                DeviceState::Unauthorized => {
                    return Err(format!(
                        "{serial} has not allowed USB debugging from this computer; ask the person to accept the prompt on the phone"
                    ));
                }
                DeviceState::NotFound(detail) if !is_avd => {
                    return Err(not_ready_error(&serial, &detail));
                }
                DeviceState::NotFound(detail) | DeviceState::Transient(detail) => {
                    if is_avd {
                        match refresh() {
                            Some(current) => serial = current,
                            None => {
                                return Err(format!(
                                    "{missing_label} is not running. Call sim attach to start it, or ask the person."
                                ));
                            }
                        }
                    }
                    let until = *deadline.get_or_insert_with(|| Instant::now() + wait);
                    if Instant::now() >= until {
                        return Err(not_ready_error(&serial, &detail));
                    }
                    crate::pause(SERIAL_READY_STEP).await?;
                }
            }
        }
    }

    /// The program on the device that reads its screen and moves its fingers, started on first use.
    async fn session(&self, serial: &str) -> SimResult<(SimManager, Program)> {
        let program = Program {
            path: self.adb_path()?,
            args: [
                "-s",
                serial,
                "shell",
                &format!("CLASSPATH={DEVICE_JAR} exec app_process / com.sikemux.device.Main"),
            ]
            .map(Into::into)
            .to_vec(),
        };
        let existing = self.sessions.locked().get(serial).cloned();
        if let Some(session) = existing {
            return Ok((session, program));
        }
        self.push_program(serial).await?;
        let session = SimManager::default();
        self.sessions
            .locked()
            .insert(serial.to_owned(), session.clone());
        Ok((session, program))
    }

    async fn push_program(&self, serial: &str) -> SimResult<()> {
        use tokio::io::AsyncWriteExt;
        let adb = self.adb_path()?;
        let mut child = sikemux_process::command::Command::into_async(
            sikemux_process::user_environment::command(adb),
        )
        .args(["-s", serial, "exec-in", &format!("cat > {DEVICE_JAR}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not run adb: {error}"))?;
        let mut stdin = child.stdin.take().ok_or("adb took no input")?;
        stdin
            .write_all(DEVICE_PROGRAM)
            .await
            .map_err(|error| format!("could not copy Sikemux's program to the device: {error}"))?;
        drop(stdin);
        child.wait().await.map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Asks the program on the device, copying it there again once if the device lost it.
    /// Drains the session on timeout or cancellation so a hung request never wedges later calls.
    async fn device(
        &self,
        serial: &str,
        kind: &str,
        mut fields: Map<String, Value>,
        limit: Duration,
    ) -> SimResult<Value> {
        struct DrainOnDrop<'a> {
            session: &'a SimManager,
            armed: bool,
        }
        impl Drop for DrainOnDrop<'_> {
            fn drop(&mut self) {
                if self.armed {
                    self.session.drain();
                }
            }
        }
        fields.insert("type".into(), kind.into());
        let (session, program) = self.session(serial).await?;
        let mut guard = DrainOnDrop {
            session: &session,
            armed: true,
        };
        let answer = match session
            .call_within(program.clone(), fields.clone(), limit)
            .await
        {
            Err(error) if error.contains("helper stopped") => {
                session.drain();
                self.push_program(serial).await?;
                session.call_within(program, fields, limit).await
            }
            answer => answer,
        };
        if !answer
            .as_ref()
            .is_err_and(|error| error.contains("in time"))
        {
            guard.armed = false;
        }
        answer
    }

    async fn display(&self, serial: &str) -> SimResult<Display> {
        if let Some(known) = self.displays.locked().get(serial) {
            return Ok(*known);
        }
        let size = self.shell(serial, "wm size").await?;
        let density = self.shell(serial, "wm density").await?;
        let last = |text: &str, wanted: &str| {
            text.lines()
                .filter(|line| line.contains(wanted))
                .filter_map(|line| line.rsplit(':').next().map(str::trim).map(str::to_owned))
                .next_back()
        };
        let (width, height) = last(&size, "size")
            .and_then(|size| {
                let (width, height) = size.split_once('x')?;
                Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?))
            })
            .ok_or("the device did not say how large its screen is")?;
        let density: f64 = last(&density, "density")
            .and_then(|density| density.parse().ok())
            .unwrap_or(160.);
        let display = Display {
            width,
            height,
            scale: density / 160.,
        };
        self.displays.locked().insert(serial.to_owned(), display);
        Ok(display)
    }

    /// The screen's corner radius as a share of its short side, as Android rounds the corners
    /// it draws over the picture.
    async fn corner(&self, udid: &str) -> Option<f64> {
        let serial = self.serial(udid).await.ok()?;
        let display = self.display(&serial).await.ok()?;
        let dump = self.shell(&serial, "dumpsys display").await.ok()?;
        let radius: f64 = dump
            .split("RoundedCorner{")
            .nth(1)?
            .split("radius=")
            .nth(1)?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some(radius / display.width.min(display.height))
    }

    async fn launcher(&self, serial: &str) -> Option<String> {
        if let Some(known) = self.launchers.locked().get(serial) {
            return known.clone();
        }
        let found = self
            .shell(serial, "cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.HOME")
            .await
            .ok()
            .and_then(|text| text.lines().last().and_then(|line| line.split('/').next()).map(str::to_owned))
            .filter(|package| package.contains('.'));
        self.launchers
            .locked()
            .insert(serial.to_owned(), found.clone());
        found
    }

    /// Answers one request, as the iOS helper would answer it.
    pub async fn request(
        &self,
        kind: &str,
        fields: Map<String, Value>,
        limit: Duration,
    ) -> SimResult<Value> {
        let udid = fields
            .get("udid")
            .and_then(Value::as_str)
            .ok_or("an Android request needs a device")?
            .to_owned();
        let text = |key: &str| fields.get(key).and_then(Value::as_str);
        let number = |key: &str| fields.get(key).and_then(Value::as_f64);
        match kind {
            "boot" => self.boot(&udid, limit).await,
            "shutdown" => {
                let serial = self.serial(&udid).await?;
                self.forget(&serial);
                if serial.starts_with("emulator-") {
                    self.flush_emulator_disk(&serial).await;
                    self.adb(&["-s", &serial, "emu", "kill"], Duration::from_secs(30))
                        .await?;
                }
                Ok(json!({}))
            }
            "chrome" => Ok(json!({ "chrome": null })),
            "mask" => Ok(json!({ "mask": null, "corner": self.corner(&udid).await })),
            "screen" => {
                let serial = self.serial(&udid).await?;
                let display = self.display(&serial).await?;
                let orientation = self.orientation_now(&serial).await;
                let (width, height) = turned(display, &orientation);
                Ok(json!({
                    "width": (width / display.scale).round(),
                    "height": (height / display.scale).round(),
                    "scale": display.scale,
                    "orientation": orientation,
                }))
            }
            "tree" => {
                let serial = self.serial(&udid).await?;
                let display = self.display(&serial).await?;
                let read = self.device(&serial, "tree", Map::new(), limit).await?;
                let launcher = self.launcher(&serial).await;
                let package = read
                    .get("package")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let pid = self.pids.locked().get(&(serial.clone(), package)).copied();
                Ok(tree(&read, display.scale, launcher.as_deref(), pid))
            }
            "tap" => {
                let serial = self.serial(&udid).await?;
                let scale = self.display(&serial).await?.scale;
                let at = [
                    number("x").unwrap_or(0.) * scale,
                    number("y").unwrap_or(0.) * scale,
                ];
                let held = number("duration").unwrap_or(0.05).max(0.02);
                let frames = json!([{ "t": 0, "pointers": [at] }, { "t": held, "pointers": [at] }]);
                self.gesture(&serial, frames, limit).await
            }
            "swipe" => {
                let serial = self.serial(&udid).await?;
                let scale = self.display(&serial).await?.scale;
                let from = (number("x").unwrap_or(0.), number("y").unwrap_or(0.));
                let to = (
                    number("toX").unwrap_or(from.0),
                    number("toY").unwrap_or(from.1),
                );
                let duration = number("duration").unwrap_or(0.3).max(SWIPE_STEP);
                let steps = (duration / SWIPE_STEP).ceil() as usize;
                let frames: Vec<Value> = (0..=steps)
                    .map(|step| {
                        let share = step as f64 / steps as f64;
                        let x = (from.0 + (to.0 - from.0) * share) * scale;
                        let y = (from.1 + (to.1 - from.1) * share) * scale;
                        json!({ "t": duration * share, "pointers": [[x, y]] })
                    })
                    .collect();
                self.gesture(&serial, frames.into(), limit).await
            }
            "touchPath" | "touch2Path" => {
                let serial = self.serial(&udid).await?;
                let scale = self.display(&serial).await?.scale;
                let frames: Vec<Value> = fields
                    .get("points")
                    .and_then(Value::as_array)
                    .ok_or("points is required")?
                    .iter()
                    .map(|point| {
                        let finger = |x: &str, y: &str| {
                            json!([
                                point[x].as_f64().unwrap_or(0.) * scale,
                                point[y].as_f64().unwrap_or(0.) * scale
                            ])
                        };
                        let mut pointers = vec![finger("x", "y")];
                        if kind == "touch2Path" {
                            pointers.push(finger("x2", "y2"));
                        }
                        json!({ "t": point["t"].as_f64().unwrap_or(0.), "pointers": pointers })
                    })
                    .collect();
                self.gesture(&serial, frames.into(), limit).await
            }
            "touch" => {
                let serial = self.serial(&udid).await?;
                let scale = self.display(&serial).await?.scale;
                let mut asked = Map::new();
                asked.insert("phase".into(), text("phase").unwrap_or("down").into());
                asked.insert("x".into(), (number("x").unwrap_or(0.) * scale).into());
                asked.insert("y".into(), (number("y").unwrap_or(0.) * scale).into());
                self.device(&serial, "touch", asked, limit).await
            }
            "text" => {
                let serial = self.serial(&udid).await?;
                let mut asked = Map::new();
                asked.insert("text".into(), text("text").unwrap_or_default().into());
                self.device(&serial, "text", asked, limit).await
            }
            "key" => {
                let serial = self.serial(&udid).await?;
                match key_code(text("key").unwrap_or_default()) {
                    Some(code) => self.key(&serial, code, "press", limit).await,
                    None => Ok(json!({})),
                }
            }
            "button" => {
                let serial = self.serial(&udid).await?;
                let button = text("button").ok_or("button is required")?;
                let code = button_code(button).ok_or_else(|| {
                    format!("Android has no {button} button; use home, back, lock, volumeUp or volumeDown")
                })?;
                self.key(&serial, code, text("phase").unwrap_or("press"), limit)
                    .await
            }
            "screenshot" => {
                let serial = self.serial(&udid).await?;
                let display = self.display(&serial).await?;
                let saving = text("path").map(str::to_owned);
                let jpeg = saving.is_none() && text("format") == Some("jpeg");
                let point_size = fields.get("pointSize").and_then(Value::as_bool) == Some(true);
                let mut asked = Map::new();
                asked.insert("format".into(), if jpeg { "jpeg" } else { "png" }.into());
                asked.insert(
                    "scale".into(),
                    if point_size { 1. / display.scale } else { 1. }.into(),
                );
                let shot = self.device(&serial, "screenshot", asked, limit).await?;
                match saving {
                    Some(path) => save_screenshot(&shot, &path),
                    None => Ok(shot),
                }
            }
            "orientation" => {
                let serial = self.serial(&udid).await?;
                match text("orientation") {
                    Some(name) => {
                        let rotation = rotation_of(name)
                            .ok_or_else(|| format!("no orientation is called {name}"))?;
                        let mut asked = Map::new();
                        asked.insert("rotation".into(), rotation.into());
                        self.device(&serial, "rotate", asked, limit).await?;
                        self.rotations.locked().insert(serial, rotation);
                        Ok(json!({ "orientation": name }))
                    }
                    None => Ok(json!({ "orientation": self.orientation_now(&serial).await })),
                }
            }
            "launch" => {
                let serial = self.serial(&udid).await?;
                let package = text("bundleId").ok_or("bundleId is required")?;
                self.launch(&serial, package, fields.get("arguments")).await
            }
            "terminate" => {
                let serial = self.serial(&udid).await?;
                let package = text("bundleId").ok_or("bundleId is required")?;
                self.shell(&serial, &format!("am force-stop {}", quoted(package)))
                    .await?;
                self.pids.locked().remove(&(serial, package.to_owned()));
                Ok(json!({}))
            }
            "install" => {
                let serial = self.serial(&udid).await?;
                let path = text("path").ok_or("path is required")?;
                self.install(&serial, path).await
            }
            "uninstall" => {
                let serial = self.serial(&udid).await?;
                let package = text("bundleId").ok_or("bundleId is required")?;
                let output = self
                    .adb(
                        &["-s", &serial, "uninstall", package],
                        Duration::from_secs(60),
                    )
                    .await?;
                if output.contains("Failure") {
                    return Err(output.trim().to_owned());
                }
                self.pids.locked().remove(&(serial, package.to_owned()));
                Ok(json!({ "uninstalled": package }))
            }
            "openUrl" => {
                let serial = self.serial(&udid).await?;
                let url = text("url").ok_or("url is required")?;
                self.shell(
                    &serial,
                    &format!("am start -a android.intent.action.VIEW -d {}", quoted(url)),
                )
                .await?;
                Ok(json!({}))
            }
            "logs" => {
                let serial = self.serial(&udid).await?;
                let tail = self.logcat(&serial)?;
                let pids = match text("process") {
                    Some(package) => Some(self.pids_of(&serial, package).await),
                    None => None,
                };
                let limit = fields.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
                let after = fields.get("after").and_then(Value::as_u64).unwrap_or(0);
                let generation = fields.get("generation").and_then(Value::as_i64);
                Ok(tail.read(pids.as_deref(), after, limit.clamp(1, 2000), generation))
            }
            "stopLogs" => {
                if let Some(serial) = self.target(&udid).serial {
                    self.logs.locked().remove(&serial);
                }
                Ok(json!({}))
            }
            other => Err(format!("Android devices cannot {other} yet")),
        }
    }

    async fn gesture(&self, serial: &str, frames: Value, limit: Duration) -> SimResult<Value> {
        let mut asked = Map::new();
        asked.insert("frames".into(), frames);
        self.device(serial, "gesture", asked, limit).await
    }

    async fn key(&self, serial: &str, code: i64, phase: &str, limit: Duration) -> SimResult<Value> {
        let mut asked = Map::new();
        asked.insert("code".into(), code.into());
        asked.insert("phase".into(), phase.into());
        self.device(serial, "key", asked, limit).await
    }

    async fn orientation_now(&self, serial: &str) -> String {
        let rotation = self.rotations.locked().get(serial).copied();
        let rotation = match rotation {
            Some(rotation) => rotation,
            None => self
                .shell(serial, "settings get system user_rotation")
                .await
                .ok()
                .and_then(|text| text.trim().parse().ok())
                .unwrap_or(0),
        };
        orientation_name(rotation).to_owned()
    }

    async fn run_install(&self, args: &[&str]) -> SimResult<String> {
        let output = self.adb(args, Duration::from_secs(180)).await?;
        if output.contains("Failure") {
            return Err(output.trim().to_owned());
        }
        Ok(output)
    }

    async fn package_installed(&self, serial: &str, package: &str) -> bool {
        self.shell(serial, &format!("pm path {}", quoted(package)))
            .await
            .is_ok_and(|out| out.contains("package:"))
    }

    async fn wait_for_package(&self, serial: &str, package: &str) -> bool {
        if self.package_installed(serial, package).await {
            return true;
        }
        for _ in 0..PACKAGE_SETTLE_TRIES {
            tokio::time::sleep(PACKAGE_SETTLE_STEP).await;
            if self.package_installed(serial, package).await {
                return true;
            }
        }
        false
    }

    async fn flush_emulator_disk(&self, serial: &str) {
        if serial.starts_with("emulator-") {
            let _ = self
                .adb(&["-s", serial, "shell", "sync"], Duration::from_secs(5))
                .await;
        }
    }

    async fn data_space(&self, serial: &str) -> Option<DataSpace> {
        let output = self
            .adb(
                &["-s", serial, "shell", "df -k /data"],
                STORAGE_CHECK_TIMEOUT,
            )
            .await
            .ok()?;
        parse_df_k(&output)
    }

    async fn third_party_package_sizes(&self, serial: &str) -> Vec<(String, u64)> {
        let Ok(output) = self
            .adb(
                &["-s", serial, "shell", PACKAGE_SIZES_CMD],
                PACKAGE_SIZES_TIMEOUT,
            )
            .await
        else {
            return Vec::new();
        };
        parse_package_sizes(&output)
    }

    pub(crate) async fn low_storage_warning(&self, udid: &str) -> Option<String> {
        let serial = self.target(udid).serial?;
        let space = self.data_space(&serial).await?;
        low_storage_warning_for(space)
    }

    async fn storage_error(&self, serial: &str, error: &str) -> String {
        if !is_insufficient_storage(error) {
            return error.to_owned();
        }
        let (space, packages) = tokio::join!(
            self.data_space(serial),
            self.third_party_package_sizes(serial)
        );
        insufficient_storage_error(error, space, &packages)
    }

    async fn finish_install(
        &self,
        serial: &str,
        path: &str,
        answer: String,
        package: Option<String>,
        allow_clean_reinstall: bool,
    ) -> SimResult<Value> {
        let Some(package) = package else {
            self.flush_emulator_disk(serial).await;
            return Ok(json!({ "installed": answer.trim(), "bundleId": Value::Null }));
        };
        self.pids
            .locked()
            .remove(&(serial.to_owned(), package.clone()));
        if self.wait_for_package(serial, &package).await {
            self.flush_emulator_disk(serial).await;
            return Ok(json!({ "installed": answer.trim(), "bundleId": package }));
        }
        if !allow_clean_reinstall {
            return Err(format!(
                "adb install reported success, but {package} is still not installed on the device"
            ));
        }
        let _ = self
            .adb(
                &["-s", serial, "uninstall", &package],
                Duration::from_secs(60),
            )
            .await;
        let fresh = self
            .run_install(&["-s", serial, "install", "--no-streaming", "-t", "-g", path])
            .await?;
        if !self.wait_for_package(serial, &package).await {
            return Err(format!(
                "adb install reported success, but {package} is still not installed on the device after clean reinstall"
            ));
        }
        self.flush_emulator_disk(serial).await;
        Ok(json!({
            "installed": fresh.trim(),
            "bundleId": package,
            "reinstalled": true,
            "note": format!(
                "Overwrite install reported success, but {package} was missing from PackageManager; uninstalled any leftover state and reinstalled cleanly."
            ),
        }))
    }

    async fn install(&self, serial: &str, path: &str) -> SimResult<Value> {
        if !path.ends_with(".apk") {
            return Err("give an .apk, such as the one ./gradlew assembleDebug builds into app/build/outputs/apk/debug; an .aab is for the Play Store and cannot be installed directly".into());
        }
        let overwrite = self
            .run_install(&["-s", serial, "install", "-r", "-t", "-g", path])
            .await;
        match overwrite {
            Ok(answer) => {
                let package = apk_package(self.sdk().as_deref(), path).await;
                self.finish_install(serial, path, answer, package, true)
                    .await
            }
            Err(error) if is_insufficient_storage(&error) => {
                let _ = self.shell(serial, "pm trim-caches 1G").await;
                if let Ok(answer) = self
                    .run_install(&["-s", serial, "install", "-r", "-t", "-g", path])
                    .await
                {
                    let package = apk_package(self.sdk().as_deref(), path).await;
                    return self
                        .finish_install(serial, path, answer, package, true)
                        .await;
                }
                let package = apk_package(self.sdk().as_deref(), path).await;
                let installed = match package.as_deref() {
                    Some(package) => self.package_installed(serial, package).await,
                    None => false,
                };
                if let Some(package) = package.filter(|_| installed) {
                    self.adb(
                        &["-s", serial, "uninstall", &package],
                        Duration::from_secs(60),
                    )
                    .await?;
                    self.pids
                        .locked()
                        .remove(&(serial.to_owned(), package.clone()));
                    let answer = match self
                        .run_install(&["-s", serial, "install", "-t", "-g", path])
                        .await
                    {
                        Ok(answer) => answer,
                        Err(fresh) => return Err(self.storage_error(serial, &fresh).await),
                    };
                    if !self.wait_for_package(serial, &package).await {
                        return Err(format!(
                            "adb install reported success, but {package} is still not installed on the device"
                        ));
                    }
                    self.flush_emulator_disk(serial).await;
                    return Ok(json!({
                        "installed": answer.trim(),
                        "bundleId": package,
                        "reinstalled": true,
                        "note": format!(
                            "Overwrite install hit INSTALL_FAILED_INSUFFICIENT_STORAGE, so {package} was uninstalled first (clearing its app data) before reinstalling."
                        ),
                    }));
                }
                Err(self.storage_error(serial, &error).await)
            }
            Err(error) => Err(error),
        }
    }

    async fn resolve_launcher_activity(
        &self,
        serial: &str,
        package: &str,
    ) -> SimResult<Option<String>> {
        let resolved = self
            .shell(
                serial,
                &format!(
                    "cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.LAUNCHER {}",
                    quoted(package)
                ),
            )
            .await?;
        Ok(resolved
            .lines()
            .last()
            .map(str::trim)
            .filter(|line| line.contains('/'))
            .map(str::to_owned))
    }

    async fn launch(
        &self,
        serial: &str,
        package: &str,
        arguments: Option<&Value>,
    ) -> SimResult<Value> {
        let mut activity = self.resolve_launcher_activity(serial, package).await?;
        if activity.is_none() {
            if !self.package_installed(serial, package).await {
                return Err(format!(
                    "{package} is not installed; sim install its .apk first"
                ));
            }
            for _ in 0..PACKAGE_SETTLE_TRIES {
                tokio::time::sleep(PACKAGE_SETTLE_STEP).await;
                activity = self.resolve_launcher_activity(serial, package).await?;
                if activity.is_some() {
                    break;
                }
            }
        }
        let activity = activity.ok_or_else(|| {
            format!("{package} is installed, but has no launcher activity to open")
        })?;
        let previous = self.pids_of(serial, package).await;
        self.pids
            .locked()
            .remove(&(serial.to_owned(), package.to_owned()));
        self.shell(serial, &launch_command(&activity, arguments))
            .await?;
        let mut pid = None;
        for _ in 0..20 {
            let now = self.pids_of(serial, package).await;
            if let Some(fresh) = now.iter().copied().find(|pid| !previous.contains(pid)) {
                pid = Some(fresh);
                break;
            }
            pid = now.first().copied();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        if let Some(pid) = pid {
            self.pids
                .locked()
                .insert((serial.to_owned(), package.to_owned()), pid);
        }
        Ok(json!({ "pid": pid }))
    }

    async fn pids_of(&self, serial: &str, package: &str) -> Vec<i64> {
        self.shell(serial, &format!("pidof {}", quoted(package)))
            .await
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .collect()
    }

    fn logcat(&self, serial: &str) -> SimResult<Arc<Logcat>> {
        if let Some(tail) = self.logs.locked().get(serial) {
            return Ok(Arc::clone(tail));
        }
        let tail = Logcat::follow(&self.adb_path()?, serial)?;
        self.logs
            .locked()
            .insert(serial.to_owned(), Arc::clone(&tail));
        Ok(tail)
    }

    fn forget(&self, serial: &str) {
        if let Some(session) = self.sessions.locked().remove(serial) {
            session.drain();
        }
        self.displays.locked().remove(serial);
        self.rotations.locked().remove(serial);
        self.logs.locked().remove(serial);
    }

    /// Starts a virtual device with no window of its own, for the desk to show, and
    /// waits until Android has finished starting. A phone is already on.
    async fn boot(&self, udid: &str, limit: Duration) -> SimResult<Value> {
        let target = self.target(udid);
        let Some(avd) = target.avd else {
            self.serial(udid).await?;
            return Ok(json!({}));
        };
        let started = Instant::now();
        let mut emulator_exit = None;
        if target.serial.is_none() {
            let root = self.sdk().ok_or(NO_SDK)?;
            let emulator = sdk::emulator(&root);
            if !emulator.is_file() {
                return Err(format!(
                    "the Android SDK at {} has no emulator",
                    root.display()
                ));
            }
            let mut command = sikemux_process::user_environment::command(emulator);
            command
                .args([
                    "-avd",
                    &avd,
                    "-no-window",
                    "-no-boot-anim",
                    "-grpc-use-token",
                    "-netdelay",
                    "none",
                    "-netspeed",
                    "full",
                ])
                .env("ANDROID_HOME", &root)
                .env("ANDROID_SDK_ROOT", &root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            #[cfg(unix)]
            command.process_group(0);
            let child = sikemux_process::command::Command::into_async(command)
                .spawn()
                .map_err(|error| format!("could not start the emulator: {error}"))?;
            let (exited, on_exit) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let _ = exited.send(watch_emulator(child).await);
            });
            emulator_exit = Some(on_exit);
        }
        loop {
            if started.elapsed() >= limit {
                return Err(format!("{avd} did not finish starting in time"));
            }
            if let Some(Ok(why)) = emulator_exit.as_mut().map(|exit| exit.try_recv()) {
                return Err(format!(
                    "the emulator for {avd} stopped as it started: {why}"
                ));
            }
            if let Some(serial) = self.target(udid).serial {
                let ready = self
                    .adb(&["-s", &serial, "get-state"], SERIAL_PROBE_TIMEOUT)
                    .await
                    .is_ok_and(|state| state.trim() == "device");
                let booted = ready
                    && self
                        .shell(&serial, "getprop sys.boot_completed")
                        .await
                        .is_ok_and(|text| text.trim() == "1");
                if booted {
                    return Ok(json!({}));
                }
            }
            tokio::time::sleep(BOOT_POLL).await;
        }
    }

    /// Streams the emulator's screen at twice its size in points, each picture handed to `on_frame` as its width and height, little-endian, then three bytes a pixel.
    pub async fn stream(
        &self,
        udid: &str,
        mut on_frame: impl FnMut(Vec<u8>) -> bool,
    ) -> SimResult<()> {
        let serial = self.serial(udid).await?;
        let display = self.display(&serial).await?;
        if is_phone(udid) {
            return self.record(udid, &serial, display, on_frame).await;
        }
        let side = (display.width.max(display.height) / display.scale * 2.)
            .min(display.width.max(display.height)) as u32;
        let target = self.target(udid);
        let (port, token) = target.grpc.ok_or(
            "this device's screen cannot stream to the desk yet: only emulators Sikemux starts, or that run with -grpc-use-token, can",
        )?;
        grpc::stream_screenshot(port, &token, side, side, |image| {
            let mut frame = Vec::with_capacity(8 + image.pixels.len());
            frame.extend_from_slice(&image.width.to_le_bytes());
            frame.extend_from_slice(&image.height.to_le_bytes());
            frame.extend_from_slice(&image.pixels);
            on_frame(frame)
        })
        .await
    }

    /// Streams a phone's screen as its own hardware encoder makes it, H.264 frames as the iOS
    /// helper sends them, at twice its size in points. `screenrecord` stops after three minutes,
    /// so it is started again until the phone goes away.
    async fn record(
        &self,
        udid: &str,
        serial: &str,
        display: Display,
        mut on_frame: impl FnMut(Vec<u8>) -> bool,
    ) -> SimResult<()> {
        use tokio::io::AsyncReadExt;
        let shrink = (2. / display.scale).min(1.);
        let even = |side: f64| ((side * shrink) as u32) & !1;
        let size = format!("{}x{}", even(display.width), even(display.height));
        loop {
            let mut child = sikemux_process::command::Command::into_async(
                sikemux_process::user_environment::command(self.adb_path()?),
            )
            .args([
                "-s",
                serial,
                "exec-out",
                "screenrecord",
                "--output-format=h264",
            ])
            .args(["--size", &size, "--bit-rate", "8000000", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("could not record the phone's screen: {error}"))?;
            let mut output = child.stdout.take().ok_or("the recording has no output")?;
            let mut frames = Frames::default();
            let mut chunk = vec![0; 1 << 16];
            loop {
                let read = output
                    .read(&mut chunk)
                    .await
                    .map_err(|error| error.to_string())?;
                if read == 0 {
                    break;
                }
                for frame in frames.push(chunk.get(..read).unwrap_or_default()) {
                    if !on_frame(frame) {
                        return Ok(());
                    }
                }
            }
            if self.serial(udid).await.is_err() {
                return Err("the phone went away".into());
            }
        }
    }

    /// Lets go of every device as Sikemux quits.
    pub fn drain(&self) {
        for (_, session) in self.sessions.locked().drain() {
            session.drain();
        }
        self.logs.locked().clear();
    }

    /// Shuts down the emulators named, as Sikemux quits; phones stay as they are.
    pub fn shut_down(&self, udids: &[String]) {
        let Ok(adb) = self.adb_path() else {
            return;
        };
        for udid in udids {
            if let Some(serial) = self
                .target(udid)
                .serial
                .filter(|_| udid.starts_with(VIRTUAL))
            {
                let _ = sikemux_process::user_environment::command(&adb)
                    .args(["-s", &serial, "emu", "kill"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
        }
    }
}

/// Waits for an emulator to exit, reading its stderr all the while so it never blocks on a
/// full pipe, and says how it ended with the last of what it wrote.
async fn watch_emulator(mut child: sikemux_process::command::AsyncChild) -> String {
    use tokio::io::AsyncReadExt;
    const TAIL: usize = 2048;
    enum Event {
        Exited(std::io::Result<std::process::ExitStatus>),
        Read(Option<usize>),
    }
    let mut stderr = child.stderr.take();
    let mut tail = Vec::new();
    let mut chunk = [0; 1024];
    let status = loop {
        let event = tokio::select! {
            status = child.wait() => Event::Exited(status),
            read = async { stderr.as_mut()?.read(&mut chunk).await.ok() }, if stderr.is_some() => Event::Read(read),
        };
        match event {
            Event::Exited(status) => break status,
            Event::Read(Some(read @ 1..)) => {
                tail.extend_from_slice(chunk.get(..read).unwrap_or_default());
            }
            Event::Read(_) => stderr = None,
        }
        tail.drain(..tail.len().saturating_sub(TAIL));
    };
    if let Some(mut stderr) = stderr {
        let _ = tokio::time::timeout(Duration::from_secs(1), stderr.read_to_end(&mut tail)).await;
    }
    let status = status.map_or_else(|error| error.to_string(), |status| status.to_string());
    let written = String::from_utf8_lossy(
        tail.get(tail.len().saturating_sub(TAIL)..)
            .unwrap_or_default(),
    );
    match written.trim() {
        "" => status,
        written => format!("{status}\n{written}"),
    }
}

pub(crate) const NO_SDK: &str = "The Android emulator is not set up on this computer. Ask the person to set it up from the desk's device menu or in Settings, under Android Emulator.";

fn turned(display: Display, orientation: &str) -> (f64, f64) {
    if orientation.starts_with("landscape") {
        (display.height, display.width)
    } else {
        (display.width, display.height)
    }
}

fn rotation_of(name: &str) -> Option<u8> {
    match name {
        "portrait" => Some(0),
        "landscapeLeft" => Some(1),
        "portraitUpsideDown" => Some(2),
        "landscapeRight" => Some(3),
        _ => None,
    }
}

fn orientation_name(rotation: u8) -> &'static str {
    match rotation {
        1 => "landscapeLeft",
        2 => "portraitUpsideDown",
        3 => "landscapeRight",
        _ => "portrait",
    }
}

fn key_code(key: &str) -> Option<i64> {
    Some(match key {
        "Enter" => 66,
        "Backspace" => 67,
        "Delete" => 112,
        "Escape" => 111,
        "ArrowUp" => 19,
        "ArrowDown" => 20,
        "ArrowLeft" => 21,
        "ArrowRight" => 22,
        "Tab" => 61,
        _ => return None,
    })
}

fn button_code(button: &str) -> Option<i64> {
    Some(match button {
        "home" => 3,
        "back" => 4,
        "lock" => 26,
        "volumeUp" => 24,
        "volumeDown" => 25,
        _ => return None,
    })
}

/// The Android release an API level shipped as.
pub(crate) fn android_version(api: &str) -> String {
    let level: u32 = api
        .split('.')
        .next()
        .and_then(|level| level.parse().ok())
        .unwrap_or(0);
    let release = match level {
        0..=27 => return format!("Android API {api}"),
        28 => "9".to_owned(),
        29 => "10".to_owned(),
        30 => "11".to_owned(),
        31 => "12".to_owned(),
        32 => "12L".to_owned(),
        level => (level - 20).to_string(),
    };
    format!("Android {release}")
}

/// One argument for the device's shell, quoted so nothing in it is read as shell syntax.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn launch_command(activity: &str, arguments: Option<&Value>) -> String {
    let extras: String = arguments
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|argument| format!(" {}", quoted(argument)))
        .collect();
    format!("am start -S -n {}{extras}", quoted(activity))
}

#[derive(Debug, PartialEq, Eq)]
enum DeviceState {
    Ready,
    Unauthorized,
    NotFound(String),
    Transient(String),
}

fn classify_device_state(result: Result<String, String>) -> DeviceState {
    match result {
        Ok(output) => match output.trim() {
            "device" => DeviceState::Ready,
            "unauthorized" => DeviceState::Unauthorized,
            other => DeviceState::Transient(other.to_owned()),
        },
        Err(error) => {
            let lower = error.to_ascii_lowercase();
            if lower.contains("unauthorized") {
                DeviceState::Unauthorized
            } else if lower.contains("not found") || lower.contains("no devices/emulators found") {
                DeviceState::NotFound(error)
            } else {
                DeviceState::Transient(error)
            }
        }
    }
}

fn not_ready_error(serial: &str, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() {
        format!("{serial} is not ready; it may still be starting, or be unplugged")
    } else {
        format!("{serial} is not ready; it may still be starting, or be unplugged ({detail})")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DataSpace {
    total_kb: u64,
    used_kb: u64,
    available_kb: u64,
    used_percent: u8,
}

impl DataSpace {
    fn total_mb(self) -> u64 {
        self.total_kb / 1024
    }

    fn available_mb(self) -> u64 {
        self.available_kb / 1024
    }
}

fn parse_df_k(output: &str) -> Option<DataSpace> {
    let tokens: Vec<&str> = output
        .lines()
        .skip(1)
        .flat_map(str::split_whitespace)
        .collect();
    for idx in 0..tokens.len() {
        let Some(total_kb) = tokens.get(idx).and_then(|value| value.parse::<u64>().ok()) else {
            continue;
        };
        let Some(used_kb) = tokens
            .get(idx + 1)
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(available_kb) = tokens
            .get(idx + 2)
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if total_kb == 0 {
            return None;
        }
        let used_percent = tokens
            .get(idx + 3)
            .and_then(|value| value.strip_suffix('%'))
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or_else(|| ((used_kb.saturating_mul(100)) / total_kb).min(100) as u8);
        return Some(DataSpace {
            total_kb,
            used_kb,
            available_kb,
            used_percent,
        });
    }
    None
}

fn low_storage_warning_for(space: DataSpace) -> Option<String> {
    if space.available_kb >= LOW_DATA_KB {
        return None;
    }
    Some(format!(
        "Low /data storage: {} MB free ({}% used); APK installs may fail. Free space with sim uninstall <bundleId> on unused apps.",
        space.available_mb(),
        space.used_percent,
    ))
}

fn parse_package_sizes(output: &str) -> Vec<(String, u64)> {
    let mut packages: Vec<(String, u64)> = output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let kb = parts.next()?.parse::<u64>().ok()?;
            let package = parts.next()?.trim();
            (!package.is_empty()).then(|| (package.to_owned(), kb))
        })
        .collect();
    packages.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    packages
}

fn format_package_sizes(packages: &[(String, u64)]) -> String {
    let mut shown: Vec<String> = packages
        .iter()
        .take(MAX_REPORTED_PACKAGES)
        .map(|(package, kb)| match *kb {
            0 => package.clone(),
            1..1024 => format!("{package} (<1 MB)"),
            kb => format!("{package} ({} MB)", (kb + 512) / 1024),
        })
        .collect();
    if let Some(extra) = packages.len().checked_sub(MAX_REPORTED_PACKAGES) {
        if extra > 0 {
            shown.push(format!("+{extra} more"));
        }
    }
    shown.join(", ")
}

fn is_insufficient_storage(error: &str) -> bool {
    error.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE")
}

fn insufficient_storage_error(
    error: &str,
    space: Option<DataSpace>,
    packages: &[(String, u64)],
) -> String {
    if !is_insufficient_storage(error) {
        return error.to_owned();
    }
    let mut detail = match space {
        Some(space) => format!(
            "/data has {} MB free ({}% used of {} MB) even after trimming caches",
            space.available_mb(),
            space.used_percent,
            space.total_mb(),
        ),
        None => "/data partition is full even after trimming caches".to_owned(),
    };
    if !packages.is_empty() {
        detail.push_str("; largest third-party apps (APK/code): ");
        detail.push_str(&format_package_sizes(packages));
    }
    detail.push_str(
        ". Free space with sim uninstall <bundleId> on unused apps, or wipe/recreate the AVD with a larger data partition in Android Studio.",
    );
    format!("{error} ({detail})")
}

fn save_screenshot(shot: &Value, path: &str) -> SimResult<Value> {
    use base64::Engine;
    let path = sikemux_process::paths::expand_home(path);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(shot["png"].as_str().unwrap_or_default())
        .map_err(|error| error.to_string())?;
    std::fs::write(&path, &bytes).map_err(|error| {
        format!(
            "could not save the screenshot to {}: {error}",
            path.display()
        )
    })?;
    Ok(
        json!({ "path": path, "bytes": bytes.len(), "width": shot["width"], "height": shot["height"] }),
    )
}

/// The package an .apk installs, read with the SDK's aapt or aapt2 when present.
async fn apk_package(root: Option<&Path>, apk: &str) -> Option<String> {
    let tools = root?.join("build-tools");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(tools)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    let binary = |name: &str| {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        versions
            .iter()
            .rev()
            .map(|folder| folder.join(&file))
            .find(|path| path.is_file())
    };
    if let Some(aapt) = binary("aapt") {
        let output = sikemux_process::command::Command::into_async(
            sikemux_process::user_environment::command(aapt),
        )
        .args(["dump", "badging", apk])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(name) = text
            .lines()
            .find(|line| line.starts_with("package:"))
            .and_then(|line| line.split("name='").nth(1))
            .and_then(|rest| rest.split('\'').next())
            .filter(|name| !name.is_empty())
        {
            return Some(name.to_owned());
        }
    }
    let aapt2 = binary("aapt2")?;
    let output = sikemux_process::command::Command::into_async(
        sikemux_process::user_environment::command(aapt2),
    )
    .args(["dump", "packagename", apk])
    .stdin(Stdio::null())
    .stderr(Stdio::null())
    .output()
    .await
    .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Turns the device program's read into the iOS helper's shape: the app in front,
/// holding its elements in points.
fn tree(read: &Value, scale: f64, launcher: Option<&str>, pid: Option<i64>) -> Value {
    let package = read["package"].as_str().unwrap_or_default();
    let frame = |bounds: &Value| {
        let side = |index: usize| bounds[index].as_f64().unwrap_or(0.) / scale;
        json!({ "x": side(0), "y": side(1), "width": side(2) - side(0), "height": side(3) - side(1) })
    };
    let children: Vec<Value> = read["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| element(node, frame(&node["bounds"])))
        .collect();
    if package.is_empty() {
        return json!({ "elements": children });
    }
    let label = if Some(package) == launcher {
        "Home Screen"
    } else {
        package
    };
    json!({
        "elements": [{
            "type": "Application",
            "label": label,
            "pid": pid,
            "frame": frame(&read["bounds"]),
            "children": children,
        }]
    })
}

fn element(node: &Value, frame: Value) -> Option<Value> {
    let text = |key: &str| node[key].as_str().unwrap_or_default();
    let flag = |key: &str| node[key].as_bool() == Some(true);
    let class = text("class").rsplit('.').next().unwrap_or_default();
    let editable = flag("editable");
    let role = if editable {
        if flag("password") {
            "SecureTextField"
        } else {
            "TextField"
        }
    } else if class.contains("Switch") || class == "ToggleButton" {
        "Switch"
    } else if class.contains("CheckBox") {
        "CheckBox"
    } else if class.contains("RadioButton") {
        "RadioButton"
    } else if class.contains("SeekBar") || class.contains("Slider") {
        "Slider"
    } else if class.contains("Button") || flag("clickable") {
        "Button"
    } else if class.contains("Image") {
        "Image"
    } else if flag("scrollable") {
        "ScrollView"
    } else if class.contains("Text") {
        "StaticText"
    } else {
        "GenericElement"
    };
    let (description, words) = (text("description"), text("text"));
    let (label, value) = if editable {
        let label = [description, text("hint")]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or_default();
        (label.to_owned(), words.to_owned())
    } else if flag("checkable") {
        let label = if words.is_empty() { description } else { words };
        (
            label.to_owned(),
            if flag("checked") { "1" } else { "0" }.to_owned(),
        )
    } else {
        (
            if description.is_empty() {
                words
            } else {
                description
            }
            .to_owned(),
            if description.is_empty() {
                String::new()
            } else {
                words.to_owned()
            },
        )
    };
    let identifier = text("id").rsplit(":id/").next().unwrap_or_default();
    let reachable = editable || flag("clickable") || flag("checkable") || flag("scrollable");
    if label.is_empty() && value.is_empty() && !reachable {
        return None;
    }
    let side = |name: &str| frame.get(name).and_then(Value::as_f64).unwrap_or(0.);
    if side("width") <= 0. || side("height") <= 0. {
        return None;
    }
    Some(json!({
        "type": role,
        "label": label,
        "value": value,
        "identifier": identifier,
        "enabled": flag("enabled"),
        "focused": flag("focused"),
        "frame": frame,
    }))
}

pub(crate) const NO_DEVICE: &str = "this computer has no Android device yet: no virtual device, and no phone plugged in with USB debugging on. Ask the person to set up the Android Emulator from the desk's device menu, or to plug in a phone.";

/// Cuts an H.264 byte stream into frames, each the units up to and including one picture.
#[derive(Default)]
struct Frames {
    buffer: Vec<u8>,
    scanned: usize,
    /// Where the unit being read starts, at its start code.
    unit: Option<usize>,
}

impl Frames {
    fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        let mut at = self.scanned;
        while at + 3 <= self.buffer.len() {
            if self.buffer.get(at..at + 3) != Some(&[0, 0, 1][..]) {
                at += 1;
                continue;
            }
            let start = match at.checked_sub(1) {
                Some(before) if self.buffer.get(before) == Some(&0) => before,
                _ => at,
            };
            if let Some(unit) = self.unit {
                let header = self
                    .buffer
                    .get(unit..)
                    .and_then(|rest| rest.iter().skip_while(|&&byte| byte != 1).nth(1))
                    .map(|byte| byte & 0x1f);
                if matches!(header, Some(1 | 5)) {
                    frames.push(self.buffer.drain(..start).collect());
                    at -= start;
                    self.unit = Some(0);
                    at += 3;
                    continue;
                }
            }
            self.unit = Some(start);
            at += 3;
        }
        self.scanned = at;
        frames
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn write_exec(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().expect("parent dir")).expect("create dir");
        std::fs::write(path, body).expect("write script");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn fake_sdk(adb_script: &str) -> (TempDir, Android) {
        let dir = tempfile::tempdir().expect("temp dir");
        write_exec(&dir.path().join("platform-tools/adb"), adb_script);
        write_exec(
            &dir.path().join("build-tools/37.0.0/aapt"),
            "#!/bin/sh\necho \"package: name='com.swishx.academy' versionCode='1'\"\n",
        );
        let android = Android::with_sdk(dir.path().to_path_buf());
        (dir, android)
    }

    fn fields(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("json object")
    }

    #[tokio::test]
    async fn timeout_drains_android_session_so_follow_up_calls_are_not_wedged() {
        let log_dir = tempfile::tempdir().unwrap();
        let hang_once = log_dir.path().join("hang-once");
        std::fs::write(&hang_once, "1").unwrap();
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  exec-in) cat >/dev/null ;;
  shell)
    shift
    case "$*" in
      "wm size") echo "Physical size: 1080x2400" ;;
      "wm density") echo "Physical density: 480" ;;
      *"resolve-activity"*"HOME"*) echo "com.android.launcher/.Launcher" ;;
      *"com.sikemux.device.Main"*)
        echo '{{"type":"ready"}}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
          case "$line" in
            *'"type":"text"'*)
              if [ -e '{hang}' ]; then
                rm -f '{hang}'
                sleep 30
              fi
              echo "{{\"id\":$id,\"type\":\"result\"}}"
              ;;
            *'"type":"tree"'*)
              echo "{{\"id\":$id,\"type\":\"result\",\"package\":\"com.swishx.academy\",\"bounds\":[0,0,1080,2400],\"nodes\":[]}}"
              ;;
            *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
          esac
        done
        ;;
    esac
    ;;
esac
"#,
            hang = hang_once.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        let timed_out = android
            .request(
                "text",
                fields(json!({ "udid": "adb:pixel", "text": "hello" })),
                Duration::from_millis(120),
            )
            .await
            .unwrap_err();
        assert!(timed_out.contains("in time"), "{timed_out}");

        let started = Instant::now();
        let tree = android
            .request(
                "tree",
                fields(json!({ "udid": "adb:pixel" })),
                Duration::from_secs(3),
            )
            .await
            .expect("next call after timeout must not hang behind wedged session");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "follow-up tree took {:?}",
            started.elapsed()
        );
        assert_eq!(tree["elements"][0]["label"], json!("com.swishx.academy"));
        android.drain();
    }

    #[tokio::test]
    async fn launch_force_stops_running_app_and_waits_for_fresh_pid() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("adb.log");
        let pid_state = log_dir.path().join("pid-step");
        std::fs::write(&pid_state, "0").unwrap();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  shell)
    shift
    case "$*" in
      *"resolve-activity"*"LAUNCHER"*) echo "com.swishx.academy/.MainActivity" ;;
      *"pidof"*)
        step=$(cat '{step}')
        step=$((step + 1))
        printf '%s' "$step" > '{step}'
        if [ "$step" -le 2 ]; then
          echo "1001"
        else
          echo "2002"
        fi
        ;;
      *"am start"*) echo "Stopping: com.swishx.academy" ;;
    esac
    ;;
esac
"#,
            log = log.display(),
            step = pid_state.display(),
        );
        let (_sdk, android) = fake_sdk(&script);
        let launched = android
            .request(
                "launch",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .expect("launch");
        assert_eq!(launched["pid"], json!(2002));
        let ran = std::fs::read_to_string(&log).unwrap();
        assert!(
            ran.contains("am start -S -n 'com.swishx.academy/.MainActivity'"),
            "launch must pass -S to force-stop and relaunch: {ran}"
        );
    }

    #[tokio::test]
    async fn install_trims_caches_then_uninstalls_only_on_insufficient_storage() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("adb.log");
        let state = log_dir.path().join("storage-mode");
        std::fs::write(&state, "need-uninstall").unwrap();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  shell)
    shift
    case "$*" in
      "pm trim-caches 1G")
        if [ "$(cat '{state}')" = "trim-fixes" ]; then
          printf 'ok' > '{state}'
        fi
        ;;
      *"pm path"*)
        if [ "$(cat '{state}')" != "not-installed" ]; then
          echo "package:/data/app/com.swishx.academy/base.apk"
        else
          exit 1
        fi
        ;;
    esac
    ;;
  uninstall)
    printf 'ok' > '{state}'
    echo "Success"
    ;;
  install)
    if [ "$(cat '{state}')" = "ok" ]; then
      echo "Success"
    else
      echo "adb: failed to install app.apk: Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]" >&2
      exit 1
    fi
    ;;
esac
"#,
            log = log.display(),
            state = state.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        // Case 1: overwrite fails twice even after trim-caches -> uninstalls installed pkg & reinstalls cleanly.
        let reinstalled = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-debug.apk" })),
                Duration::from_secs(10),
            )
            .await
            .expect("reinstall fallback");
        assert_eq!(reinstalled["bundleId"], json!("com.swishx.academy"));
        assert_eq!(reinstalled["reinstalled"], json!(true));
        assert!(
            reinstalled["note"]
                .as_str()
                .is_some_and(|n| n.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE")
                    && n.contains("clearing its app data")),
            "{reinstalled}"
        );

        // Case 2: trim-caches alone frees enough space -> overwrite succeeds without uninstalling.
        std::fs::write(&log, "").unwrap();
        std::fs::write(&state, "trim-fixes").unwrap();
        let preserved = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-debug.apk" })),
                Duration::from_secs(10),
            )
            .await
            .expect("trim-caches suffices");
        assert!(preserved.get("reinstalled").is_none(), "{preserved}");
        let trim_log = std::fs::read_to_string(&log).unwrap();
        assert!(trim_log.contains("pm trim-caches 1G"), "{trim_log}");
        assert!(!trim_log.contains("uninstall"), "{trim_log}");

        // Case 3: app not previously installed -> does not uninstall, returns actionable storage error.
        std::fs::write(&log, "").unwrap();
        std::fs::write(&state, "not-installed").unwrap();
        let full = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-debug.apk" })),
                Duration::from_secs(10),
            )
            .await
            .unwrap_err();
        assert!(
            full.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE") && full.contains("/data"),
            "{full}"
        );
        assert!(!std::fs::read_to_string(&log).unwrap().contains("uninstall"));

        // Case 4: explicit sim uninstall calls `adb uninstall <package>`.
        std::fs::write(&log, "").unwrap();
        let removed = android
            .request(
                "uninstall",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.example.stale" })),
                Duration::from_secs(10),
            )
            .await
            .expect("explicit uninstall");
        assert_eq!(removed["uninstalled"], json!("com.example.stale"));
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("uninstall com.example.stale"),
            "{}",
            std::fs::read_to_string(&log).unwrap()
        );
    }

    #[tokio::test]
    async fn install_verifies_package_presence_and_reinstalls_when_overwrite_drops_package() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("adb.log");
        let mode = log_dir.path().join("mode");
        let checks = log_dir.path().join("checks");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  shell)
    shift
    case "$*" in
      *"pm path"*)
        m=$(cat '{mode}')
        case "$m" in
          settle)
            c=$(cat '{checks}')
            c=$((c + 1))
            printf '%s' "$c" > '{checks}'
            if [ "$c" -ge 3 ]; then
              echo "package:/data/app/com.swishx.academy/base.apk"
            else
              exit 1
            fi
            ;;
          present) echo "package:/data/app/com.swishx.academy/base.apk" ;;
          *) exit 1 ;;
        esac
        ;;
      sync) ;;
    esac
    ;;
  uninstall)
    echo "Success"
    ;;
  install)
    m=$(cat '{mode}')
    if [ "$m" = "failure-exit-zero" ]; then
      echo "Performing Streamed Install"
      echo "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]"
      exit 0
    fi
    case "$*" in
      *"--no-streaming"*)
        if [ "$m" = "drop-then-recover" ]; then
          printf 'present' > '{mode}'
        fi
        ;;
    esac
    echo "Performing Streamed Install"
    echo "Success"
    ;;
esac
"#,
            log = log.display(),
            mode = mode.display(),
            checks = checks.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        // Case A: transient PackageManager lag after overwrite install settles without reinstalling.
        std::fs::write(&mode, "settle").unwrap();
        std::fs::write(&checks, "0").unwrap();
        let settled = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-release.apk" })),
                Duration::from_secs(10),
            )
            .await
            .expect("should settle once pm path appears");
        assert_eq!(settled["bundleId"], json!("com.swishx.academy"));
        assert!(settled.get("reinstalled").is_none(), "{settled}");
        assert!(!std::fs::read_to_string(&log).unwrap().contains("uninstall"));

        // Case B: overwrite install prints Success yet package is missing -> uninstalls leftover state and reinstalls cleanly.
        std::fs::write(&log, "").unwrap();
        std::fs::write(&mode, "drop-then-recover").unwrap();
        let recovered = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-release.apk" })),
                Duration::from_secs(10),
            )
            .await
            .expect("should recover via clean reinstall");
        assert_eq!(recovered["bundleId"], json!("com.swishx.academy"));
        assert_eq!(recovered["reinstalled"], json!(true));
        assert!(
            recovered["note"]
                .as_str()
                .is_some_and(|n| n.contains("missing from PackageManager")),
            "{recovered}"
        );
        let ran = std::fs::read_to_string(&log).unwrap();
        assert!(ran.contains("uninstall com.swishx.academy"), "{ran}");
        assert!(ran.contains("install --no-streaming -t -g"), "{ran}");

        // Case C: even after clean reinstall, package stays missing -> errors instead of lying with Success.
        std::fs::write(&log, "").unwrap();
        std::fs::write(&mode, "always-missing").unwrap();
        let failed = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-release.apk" })),
                Duration::from_secs(10),
            )
            .await
            .unwrap_err();
        assert!(failed.contains("still not installed"), "{failed}");

        // Case D: adb install exiting 0 with Failure output is rejected as an error.
        std::fs::write(&mode, "failure-exit-zero").unwrap();
        let bad = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-release.apk" })),
                Duration::from_secs(10),
            )
            .await
            .unwrap_err();
        assert!(bad.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE"), "{bad}");
    }

    #[tokio::test]
    async fn launch_waits_for_post_install_activity_resolution_and_distinguishes_missing_package() {
        let log_dir = tempfile::tempdir().unwrap();
        let mode = log_dir.path().join("mode");
        let tries = log_dir.path().join("tries");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  shell)
    shift
    case "$*" in
      *"resolve-activity"*"LAUNCHER"*)
        m=$(cat '{mode}')
        if [ "$m" = "delayed" ]; then
          t=$(cat '{tries}')
          t=$((t + 1))
          printf '%s' "$t" > '{tries}'
          if [ "$t" -ge 3 ]; then
            echo "com.swishx.academy/.MainActivity"
          else
            echo "No activity found"
          fi
        else
          echo "No activity found"
        fi
        ;;
      *"pm path"*)
        m=$(cat '{mode}')
        if [ "$m" = "not-installed" ]; then
          exit 1
        else
          echo "package:/data/app/com.swishx.academy/base.apk"
        fi
        ;;
      *"pidof"*) echo "3030" ;;
      *"am start"*) echo "Starting: Intent" ;;
    esac
    ;;
esac
"#,
            mode = mode.display(),
            tries = tries.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        // Case A: installed package whose launcher activity finishes indexing after a brief delay.
        std::fs::write(&mode, "delayed").unwrap();
        std::fs::write(&tries, "0").unwrap();
        let launched = android
            .request(
                "launch",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .expect("launch should retry when package is installed");
        assert_eq!(launched["pid"], json!(3030));

        // Case B: missing package fails fast with not-installed message.
        std::fs::write(&mode, "not-installed").unwrap();
        let started = Instant::now();
        let missing = android
            .request(
                "launch",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(missing.contains("is not installed"), "{missing}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "missing package should fail fast: {:?}",
            started.elapsed()
        );

        // Case C: installed package with no launcher activity reports that clearly.
        std::fs::write(&mode, "no-launcher").unwrap();
        let no_launcher = android
            .request(
                "launch",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(
            no_launcher.contains("has no launcher activity"),
            "{no_launcher}"
        );
    }

    #[test]
    fn df_parsing_and_low_storage_warning_threshold() {
        let android17 = "Filesystem      1K-blocks    Used Available Use% Mounted on\n\
/dev/block/dm-5   6082144 5537104    152576  98% /data/user/0\n";
        let space = parse_df_k(android17).expect("parse Android 17 df -k");
        assert_eq!(
            space,
            DataSpace {
                total_kb: 6_082_144,
                used_kb: 5_537_104,
                available_kb: 152_576,
                used_percent: 98,
            }
        );
        assert_eq!(space.available_mb(), 149);
        assert_eq!(space.total_mb(), 5939);

        let wrapped = "Filesystem           1K-blocks      Used Available Use% Mounted on\n\
/dev/block/platform/soc/1d84000.ufshc/by-name/userdata\n\
                       6082144   5537104    402828  94% /data\n";
        let wrapped_space = parse_df_k(wrapped).expect("parse wrapped df -k");
        assert_eq!(wrapped_space.available_mb(), 393);
        assert_eq!(wrapped_space.used_percent, 94);

        let warning = low_storage_warning_for(space).expect("149 MB free should warn");
        assert!(
            warning.contains("149 MB free")
                && warning.contains("98% used")
                && warning.contains("sim uninstall"),
            "{warning}"
        );

        let at_threshold = DataSpace {
            total_kb: 6_082_144,
            used_kb: 5_570_144,
            available_kb: LOW_DATA_KB,
            used_percent: 92,
        };
        assert_eq!(low_storage_warning_for(at_threshold), None);

        let just_below = DataSpace {
            available_kb: LOW_DATA_KB - 1,
            ..at_threshold
        };
        let below_warning = low_storage_warning_for(just_below).expect("499 MB free should warn");
        assert!(below_warning.contains("499 MB free"), "{below_warning}");
        assert_eq!(parse_df_k("Filesystem 1K-blocks Used Available"), None);
    }

    #[test]
    fn package_sizes_sort_format_and_enrich_storage_error() {
        let raw = "\
98144 com.swishx.academy
512 com.example.tiny
0 com.example.unknown
262144 com.nodelike.sikemux.mobile.dev
104820 com.swishx.app
204800 com.nodelike.sikemux.mobile
15360 com.example.extra
";
        let packages = parse_package_sizes(raw);
        assert_eq!(
            format_package_sizes(&packages),
            "com.nodelike.sikemux.mobile.dev (256 MB), com.nodelike.sikemux.mobile (200 MB), com.swishx.app (102 MB), com.swishx.academy (96 MB), com.example.extra (15 MB), +2 more"
        );
        assert_eq!(
            format_package_sizes(&[
                ("com.example.tiny".into(), 512),
                ("com.example.unknown".into(), 0),
            ]),
            "com.example.tiny (<1 MB), com.example.unknown"
        );

        let space = DataSpace {
            total_kb: 6_082_144,
            used_kb: 5_537_104,
            available_kb: 152_576,
            used_percent: 98,
        };
        let error = insufficient_storage_error(
            "Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]",
            Some(space),
            &packages,
        );
        assert!(
            error.contains("149 MB free")
                && error.contains("98% used")
                && error.contains("com.nodelike.sikemux.mobile.dev (256 MB)")
                && error.contains("sim uninstall <bundleId>")
                && error.contains("wipe/recreate the AVD"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn low_storage_warning_and_install_error_use_live_device_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("mode");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state) echo "device" ;;
  shell)
    shift
    case "$*" in
      "df -k /data")
        if [ "$(cat '{mode}')" = "df-fail" ]; then
          exit 1
        fi
        printf 'Filesystem      1K-blocks    Used Available Use%% Mounted on\n'
        printf '/dev/block/dm-5   6082144 5537104    152576  98%% /data/user/0\n'
        ;;
      *"pm list packages -3 -f"*)
        printf '262144 com.nodelike.sikemux.mobile.dev\n'
        printf '104820 com.swishx.app\n'
        ;;
      "pm trim-caches 1G") ;;
      *"pm path"*) exit 1 ;;
    esac
    ;;
  install)
    echo "adb: failed to install app.apk: Failure [INSTALL_FAILED_INSUFFICIENT_STORAGE]" >&2
    exit 1
    ;;
esac
"#,
            mode = mode.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        std::fs::write(&mode, "low").unwrap();
        let warning = android
            .low_storage_warning("adb:pixel")
            .await
            .expect("should warn when /data has <500 MB free");
        assert!(
            warning.contains("149 MB free") && warning.contains("98% used"),
            "{warning}"
        );

        let err = android
            .request(
                "install",
                fields(json!({ "udid": "adb:pixel", "path": "/tmp/app-debug.apk" })),
                Duration::from_secs(10),
            )
            .await
            .unwrap_err();
        assert!(
            err.contains("INSTALL_FAILED_INSUFFICIENT_STORAGE")
                && err.contains("149 MB free")
                && err.contains("com.nodelike.sikemux.mobile.dev (256 MB)")
                && err.contains("com.swishx.app (102 MB)")
                && err.contains("sim uninstall <bundleId>"),
            "{err}"
        );

        std::fs::write(&mode, "df-fail").unwrap();
        assert_eq!(android.low_storage_warning("adb:pixel").await, None);
    }

    #[tokio::test]
    async fn serial_retries_transient_blips_and_fails_fast_when_unplugged_or_unauthorized() {
        let dir = tempfile::tempdir().unwrap();
        let mode = dir.path().join("mode");
        let tries = dir.path().join("tries");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "-s" ]; then shift 2; fi
case "$1" in
  get-state)
    m=$(cat '{mode}')
    case "$m" in
      offline-then-ok)
        t=$(cat '{tries}')
        t=$((t + 1))
        printf '%s' "$t" > '{tries}'
        if [ "$t" -lt 3 ]; then
          echo "error: device offline" >&2
          exit 1
        fi
        echo "device"
        ;;
      not-found-then-ok)
        t=$(cat '{tries}')
        t=$((t + 1))
        printf '%s' "$t" > '{tries}'
        if [ "$t" -lt 3 ]; then
          echo "error: device 'emulator-5554' not found" >&2
          exit 1
        fi
        echo "device"
        ;;
      unplugged)
        t=$(cat '{tries}')
        printf '%s' "$((t + 1))" > '{tries}'
        echo "error: device 'pixel' not found" >&2
        exit 1
        ;;
      unauthorized)
        t=$(cat '{tries}')
        printf '%s' "$((t + 1))" > '{tries}'
        echo "error: device unauthorized." >&2
        exit 1
        ;;
    esac
    ;;
  shell) ;;
esac
"#,
            mode = mode.display(),
            tries = tries.display(),
        );
        let (_sdk, android) = fake_sdk(&script);

        std::fs::write(&mode, "offline-then-ok").unwrap();
        std::fs::write(&tries, "0").unwrap();
        android
            .request(
                "terminate",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .expect("terminate should ride out brief offline state");

        std::fs::write(&mode, "not-found-then-ok").unwrap();
        std::fs::write(&tries, "0").unwrap();
        let recovered = android
            .wait_for_serial_ready(
                "emulator-5554".into(),
                true,
                || Some("emulator-5554".into()),
                "Medium_Phone",
            )
            .await
            .expect("running AVD should ride out brief transport reconnect");
        assert_eq!(recovered, "emulator-5554");

        std::fs::write(&mode, "unplugged").unwrap();
        std::fs::write(&tries, "0").unwrap();
        let dead = android
            .wait_for_serial_ready("emulator-5554".into(), true, || None, "Medium_Phone")
            .await
            .unwrap_err();
        assert!(dead.contains("is not running"), "{dead}");
        assert_eq!(std::fs::read_to_string(&tries).unwrap(), "1");

        std::fs::write(&tries, "0").unwrap();
        let unplugged = android
            .request(
                "terminate",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(
            unplugged.contains("is not ready") && unplugged.contains("not found"),
            "{unplugged}"
        );
        assert_eq!(std::fs::read_to_string(&tries).unwrap(), "1");

        std::fs::write(&mode, "unauthorized").unwrap();
        std::fs::write(&tries, "0").unwrap();
        let unauth = android
            .request(
                "terminate",
                fields(json!({ "udid": "adb:pixel", "bundleId": "com.swishx.academy" })),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(unauth.contains("USB debugging"), "{unauth}");
        assert_eq!(std::fs::read_to_string(&tries).unwrap(), "1");
    }
}
