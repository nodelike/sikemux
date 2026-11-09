//! Android emulators and phones, driven through the SDK's `adb` and a small
//! program Sikemux runs on the device itself. It answers the same requests the
//! iOS helper does, in the same shapes and in device points (Android's dp), so the
//! agents' `sim_*` tools and the desk drive both alike.

mod grpc;
pub mod install;
mod logcat;
pub mod pair;
pub(crate) mod sdk;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

    pub fn sdk(&self) -> Option<PathBuf> {
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
        let mut command =
            tokio::process::Command::from(sikemux_process::user_environment::command(adb));
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
        let serial = target.serial.ok_or_else(|| {
            format!(
                "{} is not running. Call sim_attach to start it, or ask the person.",
                target.avd.as_deref().unwrap_or(udid)
            )
        })?;
        let state = self
            .adb(&["-s", &serial, "get-state"], Duration::from_secs(10))
            .await
            .unwrap_or_default();
        match state.trim() {
            "device" => Ok(serial),
            "unauthorized" => Err(format!("{serial} has not allowed USB debugging from this computer; ask the person to accept the prompt on the phone")),
            _ => Err(format!("{serial} is not ready; it may still be starting, or be unplugged")),
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
        let mut child =
            tokio::process::Command::from(sikemux_process::user_environment::command(adb))
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
    async fn device(
        &self,
        serial: &str,
        kind: &str,
        mut fields: Map<String, Value>,
        limit: Duration,
    ) -> SimResult<Value> {
        fields.insert("type".into(), kind.into());
        let (session, program) = self.session(serial).await?;
        match session
            .call_within(program.clone(), fields.clone(), limit)
            .await
        {
            Err(error) if error.contains("helper stopped") => {
                session.drain();
                self.push_program(serial).await?;
                session.call_within(program, fields, limit).await
            }
            answer => answer,
        }
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
                    self.adb(&["-s", &serial, "emu", "kill"], Duration::from_secs(30))
                        .await?;
                }
                Ok(json!({}))
            }
            "chrome" => Ok(json!({ "chrome": null })),
            "mask" => Ok(json!({ "mask": null })),
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
                let package = read["package"].as_str().unwrap_or_default().to_owned();
                let pid = self
                    .pids
                    .locked()
                    .get(&(serial.clone(), package.clone()))
                    .copied();
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
                    format!("Android has no {button} button; use home, back, appSwitch, lock, volumeUp or volumeDown")
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
                if !path.ends_with(".apk") {
                    return Err("give an .apk, such as the one ./gradlew assembleDebug builds into app/build/outputs/apk/debug; an .aab is for the Play Store and cannot be installed directly".into());
                }
                let answer = self
                    .adb(
                        &["-s", &serial, "install", "-r", "-t", "-g", path],
                        Duration::from_secs(180),
                    )
                    .await?;
                let package = apk_package(self.sdk().as_deref(), path).await;
                Ok(json!({ "installed": answer.trim(), "bundleId": package }))
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

    async fn launch(
        &self,
        serial: &str,
        package: &str,
        arguments: Option<&Value>,
    ) -> SimResult<Value> {
        let resolved = self
            .shell(serial, &format!("cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.LAUNCHER {}", quoted(package)))
            .await?;
        let activity = resolved
            .lines()
            .last()
            .map(str::trim)
            .filter(|line| line.contains('/'))
            .ok_or_else(|| format!("{package} is not installed, or has no screen to open; sim_install its .apk first"))?
            .to_owned();
        let extras: String = arguments
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|argument| format!(" {}", quoted(argument)))
            .collect();
        self.shell(
            serial,
            &format!("am start -n {}{extras}", quoted(&activity)),
        )
        .await?;
        let mut pid = None;
        for _ in 0..20 {
            pid = self.pids_of(serial, package).await.first().copied();
            if pid.is_some() {
                break;
            }
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
                .stderr(Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
            command
                .spawn()
                .map_err(|error| format!("could not start the emulator: {error}"))?;
        }
        loop {
            if started.elapsed() >= limit {
                return Err(format!("{avd} did not finish starting in time"));
            }
            if let Some(serial) = self.target(udid).serial {
                let booted = self
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
            let mut child = tokio::process::Command::from(
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
                for frame in frames.push(&chunk[..read]) {
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
        "appSwitch" => 187,
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

/// The package an .apk installs, read with the SDK's aapt when it has one.
async fn apk_package(root: Option<&Path>, apk: &str) -> Option<String> {
    let tools = root?.join("build-tools");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(tools)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    let aapt = versions
        .into_iter()
        .rev()
        .map(|folder| folder.join(if cfg!(windows) { "aapt.exe" } else { "aapt" }))
        .find(|aapt| aapt.is_file())?;
    let output = tokio::process::Command::from(sikemux_process::user_environment::command(aapt))
        .args(["dump", "badging", apk])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.starts_with("package:"))?;
    let name = line.split("name='").nth(1)?.split('\'').next()?;
    Some(name.to_owned())
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
    if frame["width"].as_f64().unwrap_or(0.) <= 0. || frame["height"].as_f64().unwrap_or(0.) <= 0. {
        return None;
    }
    Some(json!({
        "type": role,
        "label": label,
        "value": value,
        "identifier": identifier,
        "enabled": flag("enabled"),
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
            if self.buffer[at..at + 3] != [0, 0, 1] {
                at += 1;
                continue;
            }
            let start = if at > 0 && self.buffer[at - 1] == 0 {
                at - 1
            } else {
                at
            };
            if let Some(unit) = self.unit {
                let header = self.buffer[unit..]
                    .iter()
                    .position(|&byte| byte == 1)
                    .map(|one| self.buffer[unit + one + 1] & 0x1f);
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
