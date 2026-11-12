//! Agents keep running in the core after the app quits, but their desk pages and simulators live
//! in the app. Pages are written down while the app runs and as it quits, and come back unloaded
//! on every start. Simulators are handed over only across an update's restart; on any other quit
//! the devices Sikemux booted shut down.

use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{App, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_browser::SavedStrip;
use sikemux_simulator::SavedSimulators;

use crate::workbench::Workbench;

/// Older than this, the save is from an earlier restart that never came back.
const FRESH_FOR: Duration = Duration::from_secs(5 * 60);

/// How often open pages are written down, so a crash loses at most this much.
const SAVE_PAGES_EVERY: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handover {
    saved_at: u64,
    simulators: SavedSimulators,
}

/// Takes back what the last run left, and keeps this run's for the next.
pub fn follow(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    restore(bench, cx);
    keep_pages(cx);
    let handed_over = Rc::new(Cell::new(false));
    cx.on_app_restart({
        let handed_over = handed_over.clone();
        move |bench, cx| handed_over.set(save(bench, cx).is_ok())
    })
    .detach();
    cx.on_app_quit(move |bench, cx| {
        if let Some(bytes) = pages(bench, cx) {
            if let Err(error) = write(&pages_path(), &bytes) {
                eprintln!("sikemux: agents' pages were not saved: {error}");
            }
        }
        if let Some(engine) = bench.desks.read(cx).engine() {
            engine.drain("Sikemux is quitting");
        }
        #[cfg(target_os = "macos")]
        sikemux_browser::macos::save_cookies();
        bench.voice.update(cx, |voice, _| voice.shutdown());
        let simulators = bench.sims.read(cx).engine();
        if handed_over.get() {
            simulators.step_aside();
        } else {
            simulators.drain();
        }
        simulators.sim().drain();
        async {}
    })
    .detach();
}

fn handover_path() -> Option<PathBuf> {
    Some(sikemux_process::paths::config_dir()?.join("update-handover.json"))
}

fn pages_path() -> Option<PathBuf> {
    Some(sikemux_process::paths::config_dir()?.join("desk-pages.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn write(path: &Option<PathBuf>, bytes: &[u8]) -> std::io::Result<()> {
    let path = path
        .as_deref()
        .ok_or_else(|| std::io::Error::other("no config folder"))?;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no config folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn pages(bench: &Workbench, cx: &App) -> Option<Vec<u8>> {
    let strips = bench.desks.read(cx).engine()?.saved_strips();
    serde_json::to_vec(&strips).ok()
}

fn keep_pages(cx: &mut Context<Workbench>) {
    cx.spawn(async move |bench, cx| {
        let mut written = Vec::new();
        loop {
            cx.background_executor().timer(SAVE_PAGES_EVERY).await;
            let Ok(bytes) = bench.update(cx, |bench, cx| pages(bench, cx)) else {
                return;
            };
            let Some(bytes) = bytes.filter(|bytes| *bytes != written) else {
                continue;
            };
            let saving = bytes.clone();
            let saved = cx
                .background_executor()
                .spawn(async move { write(&pages_path(), &saving) })
                .await;
            match saved {
                Ok(()) => written = bytes,
                Err(error) => eprintln!("sikemux: agents' pages were not saved: {error}"),
            }
        }
    })
    .detach();
}

/// Without the save, the simulators left running would have nobody to shut them down, so a
/// failure lets them go as a plain quit does.
fn save(bench: &Workbench, cx: &App) -> std::io::Result<()> {
    let handover = Handover {
        saved_at: now(),
        simulators: bench.sims.read(cx).engine().saved(),
    };
    write(&handover_path(), &serde_json::to_vec(&handover)?).inspect_err(|error| {
        eprintln!("sikemux: simulators were not saved for the update: {error}");
    })
}

fn read_pages(path: &Path) -> Vec<SavedStrip> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn restore(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let strips = pages_path()
        .map(|path| read_pages(&path))
        .unwrap_or_default();
    let tabs = bench
        .desks
        .read(cx)
        .engine()
        .map(|engine| engine.reopen(strips))
        .unwrap_or_default();
    let devices = restore_simulators(bench, cx);
    bench
        .desks
        .update(cx, |desks, _| desks.expect(tabs.into_iter().chain(devices)));
}

fn restore_simulators(bench: &mut Workbench, cx: &mut Context<Workbench>) -> Vec<String> {
    let Some(path) = handover_path() else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    let fresh = serde_json::from_slice::<Handover>(&bytes)
        .ok()
        .filter(|handover| now().saturating_sub(handover.saved_at) <= FRESH_FOR.as_secs());
    let Some(handover) = fresh else {
        adopt_booted(bench, &bytes, cx);
        return Vec::new();
    };
    let devices = bench.sims.update(cx, |sims, _| {
        let devices = sims.engine().restore(handover.simulators);
        sims.restored();
        devices
    });
    for agent in &devices {
        bench.show_simulator(agent, cx);
    }
    devices
}

/// A save too old or too damaged to restore still names the simulators the last run booted,
/// which nobody else will ever shut down.
fn adopt_booted(bench: &Workbench, bytes: &[u8], cx: &App) {
    let Ok(saved) = serde_json::from_slice::<Value>(bytes) else {
        return;
    };
    let booted = saved["simulators"]["booted"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|udid| udid.as_str().map(str::to_owned))
        .collect();
    bench.sims.read(cx).engine().adopt_booted(booted);
}
