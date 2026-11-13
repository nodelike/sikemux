#![allow(unused_imports)]

use crate::cli::protocol::HarnessRequest;
use crate::git::Git;
use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::person_calls::Raise;
use crate::server::places::push::{self, Reach};
use crate::server::places::{self, work, Goes, Judged};
use crate::server::plugged::Provided;
use crate::server::repo_events::DeployRan;
use crate::server::repo_events::RepoEvent;
use crate::server::repo_state::RepoParts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{person_calls, plugged, Core, CoreResult};
use crate::ship::{Looks, Ship, Target, ToolShip};
use serde_json::{json, Value};
use sikemux_calls::board::Landing;
use sikemux_calls::ship::{HoldParams, WebWriteParams};
use sikemux_plugin_api::vault::{as_caller, Caller};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    CallAsker, CallKind, CallOption, CallOutcome, CallPlace, CallResult, CallState, PersonCall,
};
use sikemux_view::inbox::{ShipHold, ShipKind};
use sikemux_view::places::Recognise;
use sikemux_view::slice::Verdict;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::server::ship::*;
use crate::server::ServerCore;

/// `"ship": ["./deploy.sh"]` in a QA project's sikemux.json, and `./deploy.sh` ran with no
/// hold once the agent started it with process_start.
#[tokio::test(flavor = "multi_thread")]
async fn a_ship_command_started_as_a_process_waits_for_the_person() {
    let project = tempfile::tempdir().unwrap();
    let config = r#"{ "version": 1, "ship": ["./deploy.sh"] }"#;
    std::fs::write(project.path().join("sikemux.json"), config).unwrap();
    let core = Core::new(Default::default(), None).unwrap();
    let said = check_process(&core, "agent", "./deploy.sh", &[], project.path())
        .await
        .unwrap_err();
    assert!(said.starts_with("Waits for the person"), "{said}");
    let waiting = core.ship().waiting();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].hold.environment.as_deref(), Some("./deploy.sh"));
}
