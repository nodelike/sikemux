//! Agents keep running in the core while an update restarts the app, but their desk pages and
//! simulators live in the app. They are written down as it restarts and read back as it
//! starts; on any other quit they are let go, and the devices Sikemux booted shut down.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handover {
    saved_at: u64,
    browser: Vec<SavedStrip>,
    simulators: SavedSimulators,
}

/// Takes back what the last run handed over, and keeps this run's for the next.
pub fn follow(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    restore(bench, cx);
    let handed_over = Rc::new(Cell::new(false));
    cx.on_app_restart({
        let handed_over = handed_over.clone();
        move |bench, cx| handed_over.set(save(bench, cx).is_ok())
    })
    .detach();
    cx.on_app_quit(move |bench, cx| {
        if let Some(engine) = bench.desks.read(cx).engine() {
            engine.drain("Sikemux is quitting");
        }
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

fn path() -> Option<PathBuf> {
    Some(sikemux_process::paths::config_dir()?.join("update-handover.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Without the save, the simulators left running would have nobody to shut them down, so a
/// failure lets them go as a plain quit does.
fn save(bench: &Workbench, cx: &App) -> std::io::Result<()> {
    let handover = Handover {
        saved_at: now(),
        browser: bench
            .desks
            .read(cx)
            .engine()
            .map(|engine| engine.saved_strips())
            .unwrap_or_default(),
        simulators: bench.sims.read(cx).engine().saved(),
    };
    let path = path().ok_or_else(|| std::io::Error::other("no config folder"))?;
    let write = || {
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("no config folder"))?;
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.write_all(&serde_json::to_vec(&handover)?)?;
        file.persist(&path).map_err(|error| error.error)?;
        Ok(())
    };
    write().inspect_err(|error| {
        eprintln!("sikemux: agents' pages and simulators were not saved for the update: {error}");
    })
}

fn restore(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(path) = path() else { return };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let fresh = serde_json::from_slice::<Handover>(&bytes)
        .ok()
        .filter(|handover| now().saturating_sub(handover.saved_at) <= FRESH_FOR.as_secs());
    let Some(handover) = fresh else {
        return adopt_booted(bench, &bytes, cx);
    };
    let tabs = bench
        .desks
        .read(cx)
        .engine()
        .map(|engine| engine.reopen(handover.browser))
        .unwrap_or_default();
    let devices = bench.sims.update(cx, |sims, _| {
        let devices = sims.engine().restore(handover.simulators);
        sims.restored();
        devices
    });
    for agent in &devices {
        bench.show_simulator(agent, cx);
    }
    bench
        .desks
        .update(cx, |desks, _| desks.expect(tabs.into_iter().chain(devices)));
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
