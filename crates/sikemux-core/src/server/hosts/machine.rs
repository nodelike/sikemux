//! How much memory and how many cores each linked host has, asked once each time it is
//! reached, so an agent choosing where to send work can see which hosts are small.

use crate::server::parts::Parts;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::super::Core;
use super::ask;
use super::calls::{self as asks, params_of, HostAsk};

const ASK_WAIT: Duration = Duration::from_secs(30);
const GIB: u64 = 1 << 30;

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) struct Machine {
    memory: u64,
    cores: u32,
}

#[derive(Serialize, Deserialize)]
pub(super) struct MachineParams {}

#[derive(Default)]
pub(crate) struct Machines(Mutex<HashMap<String, Machine>>);

impl Machines {
    /// Says `host`'s size, such as `8 GB, 4 cores`, once it has said.
    pub(super) fn of(&self, host: &str) -> Option<String> {
        let machine = *self.0.locked().get(host)?;
        let memory = (machine.memory + GIB / 2) / GIB;
        Some(format!("{memory} GB, {} cores", machine.cores))
    }
}

pub(super) fn machine_here() -> Machine {
    Machine {
        memory: sikemux_process::machine::memory().map_or(0, |memory| memory.total),
        cores: std::thread::available_parallelism().map_or(1, |cores| cores.get() as u32),
    }
}

/// Asks `host` its size. A host too old to know the ask stays without one.
pub(super) fn ask_machine(core: &Arc<Core>, host: &str) {
    let (core, host) = (core.clone(), host.to_owned());
    tokio::spawn(async move {
        let params = params_of::<asks::Machine>(&MachineParams {});
        let Ok(answer) = ask(&core, &host, asks::Machine::METHOD, params, None, ASK_WAIT).await
        else {
            return;
        };
        if let Ok(machine) = serde_json::from_value::<Machine>(answer) {
            core.hosts().machines.0.locked().insert(host, machine);
        }
    });
}
