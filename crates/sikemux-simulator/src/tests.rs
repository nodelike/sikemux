use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use sikemux_process::sync::Locked;
use tempfile::TempDir;

use super::tools::Steps;
use super::SimulatorManager;

/// Tests that drive a real simulator share it, so they take turns.
static REAL_DEVICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn helper(script: &str) -> (TempDir, Arc<SimulatorManager>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("sikemux-sim");
    std::fs::write(&path, script).expect("write helper");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (dir, SimulatorManager::with_helper(path, Steps::STAND_IN))
}

/// The helper built beside the tests, as `SIKEMUX_SIM_EXECUTABLE` names it. The manager throws
/// away what it writes to stderr, so it goes to a file the test prints if it fails.
fn real_helper() -> (HelperErrors, Arc<SimulatorManager>) {
    let real = crate::local_helper().expect("SIKEMUX_SIM_EXECUTABLE names the sikemux-sim helper");
    let quoted =
        |path: &std::path::Path| format!("'{}'", path.display().to_string().replace('\'', r"'\''"));
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("sikemux-sim");
    let script = format!(
        "#!/bin/sh\nexec {} \"$@\" 2>>{}\n",
        quoted(&real),
        quoted(&dir.path().join(HELPER_ERRORS))
    );
    std::fs::write(&path, script).expect("write helper");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (
        HelperErrors(dir),
        SimulatorManager::with_helper(path, Steps::DEVICE),
    )
}

const HELPER_ERRORS: &str = "helper-stderr.log";

/// Prints what the real helper wrote to stderr when the test holding it fails.
struct HelperErrors(TempDir);

impl Drop for HelperErrors {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let errors = std::fs::read_to_string(self.0.path().join(HELPER_ERRORS))
                .unwrap_or_else(|error| format!("(none: {error})"));
            println!("the simulator helper's stderr:\n{errors}");
        }
    }
}

#[test]
#[ignore = "needs the sikemux-sim helper and Xcode's simulators"]
fn lists_the_real_simulators() {
    let _turn = REAL_DEVICE.locked();
    let (_errors, manager) = real_helper();
    let devices =
        sikemux_base::runtime::block_on(manager.request("devices", serde_json::json!({})))
            .expect("devices");
    let devices = devices["devices"].as_array().expect("a list of devices");
    assert!(!devices.is_empty(), "Xcode has no simulators");
    assert!(devices
        .iter()
        .all(|device| device["udid"].is_string() && device["state"].is_string()));
}

#[test]
fn an_agents_calls_run_one_at_a_time_and_a_cancel_stops_them() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    let manager = SimulatorManager::with_helper("/nonexistent".into(), Steps::STAND_IN);
    let order = Arc::new(Mutex::new(Vec::new()));
    let call = |name: &'static str, agent: &'static str| {
        let (manager, order) = (Arc::clone(&manager), Arc::clone(&order));
        std::thread::spawn(move || {
            sikemux_base::runtime::block_on(manager.in_turn(agent, async {
                order.lock().unwrap().push(format!("{name} start"));
                super::pause(Duration::from_millis(150)).await?;
                order.lock().unwrap().push(format!("{name} end"));
                Ok::<_, String>(())
            }))
        })
    };
    let until = |happened: &dyn Fn() -> bool| {
        while !happened() {
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let first = call("first", "agent-1");
    until(&|| !order.lock().unwrap().is_empty());
    let second = call("second", "agent-1");
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        vec!["first start", "first end", "second start", "second end"]
    );

    let at_work = Arc::new(AtomicUsize::new(0));
    let long = {
        let (manager, at_work) = (Arc::clone(&manager), Arc::clone(&at_work));
        std::thread::spawn(move || {
            sikemux_base::runtime::block_on(manager.in_turn("agent-1", async {
                at_work.fetch_add(1, Ordering::SeqCst);
                for _ in 0..100 {
                    super::pause(Duration::from_millis(50)).await?;
                }
                Ok::<_, String>(())
            }))
        })
    };
    let other = {
        let (manager, at_work) = (Arc::clone(&manager), Arc::clone(&at_work));
        std::thread::spawn(move || {
            sikemux_base::runtime::block_on(manager.in_turn("agent-2", async {
                at_work.fetch_add(1, Ordering::SeqCst);
                super::pause(Duration::from_millis(400)).await
            }))
        })
    };
    until(&|| at_work.load(Ordering::SeqCst) == 2);
    manager.cancel("agent-1");
    assert!(long.join().unwrap().unwrap_err().contains("cancelled"));
    assert!(
        other.join().unwrap().is_ok(),
        "another agent's call goes on"
    );
    let after =
        sikemux_base::runtime::block_on(manager.in_turn("agent-1", async { Ok::<_, String>(1) }));
    assert_eq!(after, Ok(1), "a call after the cancel runs");
}

mod tools {
    use serde_json::json;

    use super::{helper, real_helper, REAL_DEVICE};
    use crate::bundle::{
        app_bundle_ancestor, duplicate_scheme_conflicts_from, url_types_handle_scheme,
    };
    use crate::claims::Claim;
    use crate::load::{Machine, Wait};
    use crate::tools::{
        app_size, cap_logs, changes, choose_device, custom_url_scheme, dropped_text_warning,
        duplicate_scheme_install_warning, duplicate_scheme_open_warning, edge_warning,
        element_lines, elements_from, frontmost_bundle_id, frontmost_pid, inside_project, inspect,
        labelled, landed, launching, newest_iphone, pick_device, queued_text, run, screen_from,
        tap_point, text_fields_from, trailing_escaped_newline, type_steps,
        unhandled_custom_url_scheme_error, Device, Element, Numbered, Numbering, Pick, Read,
        TypeStep,
    };
    use crate::Queued;
    use sikemux_process::sync::Locked;
    use std::collections::{HashMap, HashSet};

    fn numbered(elements: &[Element]) -> Vec<Numbered> {
        Numbering::default().assign(elements)
    }

    fn element(role: &str, label: &str, center: (f64, f64)) -> Element {
        Element {
            role: role.into(),
            label: label.into(),
            value: String::new(),
            identifier: String::new(),
            enabled: true,
            center,
            offscreen: false,
        }
    }

    fn claim(agent: &str) -> Claim {
        Claim {
            pid: 1,
            process: String::new(),
            agent_id: agent.into(),
            project: "/work/blog".into(),
        }
    }

    /// Attaches the way an agent does when a first boot outlasts one call: by asking again.
    fn attach_when_booted(manager: &crate::SimulatorManager, agent_id: &str) -> serde_json::Value {
        let params = std::env::var("SIKEMUX_SIM_DEVICE")
            .map_or_else(|_| json!({}), |device| json!({ "device": device }));
        for _ in 0..5 {
            match run_now(manager, agent_id, "/", "sim.attach", &params) {
                Err(error) if error.contains("still booting") => continue,
                attached => return attached.unwrap_or_else(|error| panic!("sim.attach: {error}")),
            }
        }
        panic!("sim.attach: the device was still booting after five tries");
    }

    /// Runs one tool call to its end, as the harness does on its own thread.
    fn run_now(
        manager: &crate::SimulatorManager,
        agent_id: &str,
        project: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        sikemux_base::runtime::block_on(run(manager, agent_id, project, method, params))
    }

    fn device(name: &str, os: &str, booted: bool) -> Device {
        Device {
            udid: format!("{name}-{os}"),
            name: name.into(),
            os: os.into(),
            booted,
            busy: false,
            warm: booted,
            screen: Some((402.0, 874.0)),
        }
    }

    fn home_screen() -> serde_json::Value {
        let element = |kind: &str, label: &str, x: f64, y: f64| {
            json!({ "type": kind, "label": label, "value": "", "identifier": label, "enabled": true, "traits": ["LaunchIcon"],
                    "frame": { "x": x, "y": y, "width": 68, "height": 90 } })
        };
        json!({ "elements": [
            { "type": "Application", "label": " ", "pid": 7, "frame": { "x": 0, "y": 0, "width": 402, "height": 874 }, "children": [
                element("Button", "Settings", 306.0, 389.0),
                element("Button", "Safari", 24.0, 700.0),
                { "type": "GenericElement", "label": null, "frame": { "x": 0, "y": 0, "width": 10, "height": 10 } },
                { "type": "TextField", "label": "Search", "value": "cats", "enabled": false,
                  "frame": { "x": 10, "y": 10, "width": 100, "height": 30 } },
                element("Button", "Camera", 0.0, -60.0),
            ] },
        ]})
    }

    #[test]
    fn picks_the_named_device_on_the_newest_ios_and_prefers_a_booted_one() {
        let devices = [
            device("iPhone 17", "iOS 26.0", false),
            device("iPhone 17", "iOS 27.0", false),
            device("iPad Air", "iOS 27.0", true),
        ];
        assert_eq!(choose_device(&devices, "iPhone 17").unwrap().os, "iOS 27.0");
        assert_eq!(
            choose_device(&devices, "iPhone 17-iOS 26.0").unwrap().os,
            "iOS 26.0"
        );
        assert!(choose_device(&devices, "Pixel 9")
            .unwrap_err()
            .contains("sim devices"));
    }

    #[test]
    fn an_agent_without_a_device_gets_the_desk_pick_then_the_warmest_free_one() {
        let ours = device("Sikemux · shop", "iOS 27.0", false);
        let booted = device("iPhone 17", "iOS 27.0", true);
        let taken = device("iPhone 16e", "iOS 26.0", true);
        let picked = device("iPhone Air", "iOS 27.0", false);
        let devices = [ours.clone(), booted.clone(), taken.clone(), picked.clone()];
        let held: HashMap<String, Claim> = [(taken.udid.clone(), claim("agent-b"))].into();
        let pick = |desk: Option<&str>, remembered: Option<&str>, held: &HashMap<String, Claim>| {
            pick_device(
                &devices,
                held,
                desk,
                remembered,
                "Sikemux · shop",
                &HashSet::new(),
            )
        };

        assert_eq!(
            pick(Some(&picked.udid), Some(&booted.udid), &held),
            Pick::Use(&picked)
        );
        assert_eq!(pick(None, Some(&booted.udid), &held), Pick::Use(&booted));
        assert_eq!(
            pick(None, None, &held),
            Pick::Use(&booted),
            "a running iPhone before the project's own that is off"
        );
        assert_eq!(
            pick(Some(&taken.udid), None, &held),
            Pick::Use(&booted),
            "a desk pick another agent holds is passed over"
        );
        let without_booted = [ours.clone(), taken.clone(), picked.clone()];
        assert_eq!(
            pick_device(
                &without_booted,
                &held,
                None,
                None,
                "Sikemux · shop",
                &HashSet::new()
            ),
            Pick::Use(&ours)
        );
        let all_held: HashMap<String, Claim> = [ours, booted.clone(), taken.clone()]
            .iter()
            .map(|device| (device.udid.clone(), claim("agent-b")))
            .collect();
        assert_eq!(
            pick_device(
                &devices[..3],
                &all_held,
                None,
                None,
                "Sikemux · shop",
                &HashSet::new()
            ),
            Pick::Create
        );
        let without_ours = [booted.clone(), taken];
        assert_eq!(
            pick_device(
                &without_ours,
                &held,
                None,
                None,
                "Sikemux · shop",
                &HashSet::new()
            ),
            Pick::Use(&booted),
            "a booted iPhone nobody holds"
        );
        assert_eq!(
            pick_device(
                std::slice::from_ref(&picked),
                &held,
                None,
                None,
                "Sikemux · shop",
                &HashSet::new(),
            ),
            Pick::Create,
            "an iPhone that is off is not taken; the project gets its own"
        );
    }

    #[test]
    fn a_new_device_is_the_newest_iphone_on_the_newest_ios() {
        let runtimes = json!({ "runtimes": [
            { "identifier": "ios26", "version": "26.4", "platform": "iOS", "isAvailable": true,
              "supportedDeviceTypes": [{ "identifier": "iphone16", "productFamily": "iPhone" }] },
            { "identifier": "ios27", "version": "27.0", "platform": "iOS", "isAvailable": true,
              "supportedDeviceTypes": [
                { "identifier": "ipad", "productFamily": "iPad" },
                { "identifier": "iphone18", "productFamily": "iPhone" },
                { "identifier": "iphone17", "productFamily": "iPhone" } ] },
            { "identifier": "ios28", "version": "28.0", "platform": "iOS", "isAvailable": false,
              "supportedDeviceTypes": [{ "identifier": "iphone19", "productFamily": "iPhone" }] },
            { "identifier": "watch", "version": "30.0", "platform": "watchOS", "isAvailable": true,
              "supportedDeviceTypes": [] },
        ]});
        assert_eq!(
            newest_iphone(&runtimes),
            Some(("iphone18".into(), "ios27".into()))
        );
        assert_eq!(newest_iphone(&json!({ "runtimes": [] })), None);
    }

    #[test]
    fn numbers_the_elements_a_person_could_act_on() {
        let (app, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        assert_eq!(app, "Home Screen");
        assert_eq!(app_size(&home_screen()), Some((402.0, 874.0)));
        assert_eq!(
            element_lines(&numbered(&elements)),
            vec![
                "0 Button \"Settings\" at (340, 434)",
                "1 Button \"Safari\" at (58, 745)",
                "2 TextField \"Search\" value=\"cats\" [disabled] at (60, 25)",
                "3 Button \"Camera\" [offscreen] at (34, -15)",
            ]
        );
    }

    #[test]
    fn taps_an_element_by_number_or_a_point() {
        let (_, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        let elements = numbered(&elements);
        assert_eq!(tap_point(&elements, Some(1), None, None), Ok((58.0, 745.0)));
        assert_eq!(
            tap_point(&elements, None, Some(3.0), Some(4.0)),
            Ok((3.0, 4.0))
        );
        assert!(tap_point(&elements, Some(9), None, None)
            .unwrap_err()
            .contains("sim state"));
        assert!(tap_point(&elements, None, Some(3.0), None).is_err());
    }

    #[test]
    fn an_element_keeps_its_number_while_it_stays_on_screen() {
        let mut numbers = Numbering::default();
        let first = numbers.assign(&[
            element("Button", "Back", (30.0, 60.0)),
            element("Cell", "Wi-Fi", (200.0, 200.0)),
            element("Cell", "Bluetooth", (200.0, 250.0)),
        ]);
        assert_eq!(
            first.iter().map(|(number, _)| *number).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        let banner = numbers.assign(&[
            element("Button", "Back", (30.0, 60.0)),
            element("StaticText", "Update available", (200.0, 150.0)),
            element("Cell", "Wi-Fi", (200.0, 250.0)),
            element("Cell", "Bluetooth", (200.0, 300.0)),
        ]);
        assert_eq!(
            banner.iter().map(|(number, _)| *number).collect::<Vec<_>>(),
            vec![0, 3, 1, 2],
            "an element appearing above others does not renumber them"
        );
        assert_eq!(tap_point(&banner, Some(1), None, None), Ok((200.0, 250.0)));

        let gone = numbers.assign(&[
            element("Button", "Back", (30.0, 60.0)),
            element("Cell", "Bluetooth", (200.0, 300.0)),
            element("Cell", "Cellular", (200.0, 350.0)),
        ]);
        assert_eq!(
            gone.iter().map(|(number, _)| *number).collect::<Vec<_>>(),
            vec![0, 2, 4],
            "numbers are never reused"
        );
        assert!(tap_point(&gone, Some(1), None, None)
            .unwrap_err()
            .contains("not on the screen now"));

        let twins = numbers.assign(&[
            element("Button", "Delete", (300.0, 100.0)),
            element("Button", "Delete", (300.0, 400.0)),
        ]);
        let again = numbers.assign(&[
            element("Button", "Delete", (300.0, 405.0)),
            element("Button", "Delete", (300.0, 100.0)),
        ]);
        assert_eq!(
            again[0].0, twins[1].0,
            "alike elements are told apart by place"
        );
        assert_eq!(again[1].0, twins[0].0);
    }

    #[test]
    fn an_alert_ios_draws_is_the_system_not_the_home_screen() {
        let alert = json!({ "elements": [
            { "type": "Application", "label": " ", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 } },
            { "type": "Button", "label": "Allow Once", "traits": [], "frame": { "x": 56, "y": 458, "width": 290, "height": 48 } },
        ]});
        let (app, elements) = elements_from(&alert, Some((402.0, 874.0)));
        assert_eq!(app, "System");
        assert_eq!(
            element_lines(&numbered(&elements)),
            vec!["0 Button \"Allow Once\" at (201, 482)"]
        );
        let maps = json!({ "elements": [{ "type": "Application", "label": "Maps", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 } }] });
        assert_eq!(elements_from(&maps, None).0, "Maps");
    }

    #[test]
    fn shows_text_as_a_person_reads_it() {
        let safari = json!({ "elements": [
            { "type": "Application", "label": "Safari", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 } },
            { "type": "TextField", "label": "Address", "value": "\u{200e}example.com\u{200f}", "frame": { "x": 30, "y": 800, "width": 340, "height": 32 } },
            { "type": "StaticText", "label": "Say \"hi\" \\ bye", "frame": { "x": 0, "y": 100, "width": 100, "height": 20 } },
        ]});
        let (_, elements) = elements_from(&safari, Some((402.0, 874.0)));
        assert_eq!(
            element_lines(&numbered(&elements)),
            vec![
                "0 TextField \"Address\" value=\"example.com\" at (200, 816)",
                "1 StaticText \"Say \\\"hi\\\" \\\\ bye\" at (50, 110)",
            ]
        );
    }

    #[test]
    fn tells_what_appeared_changed_and_went_away() {
        let mut numbers = Numbering::default();
        let (_, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        let before = numbers.assign(&elements);
        assert_eq!(changes(&before, &before), (vec![], vec![]));

        let mut next = elements.clone();
        next[2].value = "dogs".into();
        next.remove(1);
        next.push(element("Button", "Done", (50.0, 50.0)));
        let after = numbers.assign(&next);
        let (changed, removed) = changes(&before, &after);
        assert_eq!(
            changed,
            vec![
                "2 TextField \"Search\" value=\"dogs\" [disabled] at (60, 25)",
                "4 Button \"Done\" at (50, 50)",
            ]
        );
        assert_eq!(removed, vec!["Button \"Safari\" at (58, 745)"]);
    }

    #[test]
    fn long_reads_are_capped_but_say_what_they_left_out() {
        let many: Vec<Element> = (0..250)
            .map(|n| element("Cell", &format!("Row {n}"), (200.0, n as f64 * 10.0)))
            .collect();
        let mut numbers = Numbering::default();
        let all = numbers.assign(&many);
        let lines = element_lines(&all);
        assert_eq!(lines.len(), 201);
        assert!(lines[200].contains("50 more"));

        let none = numbers.assign(&[]);
        let (changed, removed) = changes(&none, &all);
        assert_eq!(changed.len(), 201);
        assert!(changed[200].contains("50 more changed"), "{}", changed[200]);
        assert!(removed.is_empty());
        let (_, removed) = changes(&all, &none);
        assert_eq!(removed.len(), 51);
        assert!(removed[50].contains("200 more went away"));

        let mut late = all.clone();
        late[230].1.value = "on".into();
        let (changed, _) = changes(&all, &late);
        assert_eq!(
            changed,
            vec!["230 Cell \"Row 230\" value=\"on\" at (200, 2300)"],
            "a change past the 200th element is still told"
        );

        let long = element("StaticText", &"x".repeat(5000), (10.0, 10.0));
        let line = &element_lines(&numbered(&[long]))[0];
        assert!(line.len() < 300 && line.contains('…'), "{line}");
    }

    #[test]
    fn a_log_read_stays_small() {
        let mut reply = json!({
            "lines": ["a".repeat(100_000), "b".repeat(100), "c".repeat(100_000), "d"],
            "cursor": 14,
            "more": false,
        });
        cap_logs(&mut reply, 2_500);
        let lines = reply["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2, "{reply}");
        assert!(lines[0].as_str().unwrap().len() < 2_100);
        assert_eq!(
            reply["cursor"],
            json!(12),
            "the cursor stops at the last line given"
        );
        assert_eq!(reply["more"], json!(true));
    }

    #[test]
    fn an_app_to_install_must_be_inside_the_project() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(project.path().join("build/My.app")).unwrap();
        assert!(inside_project(&root, "build/My.app").is_ok());
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().to_string_lossy().into_owned();
        assert!(inside_project(&root, &elsewhere)
            .unwrap_err()
            .contains("inside the project"));
        assert!(inside_project(&root, "../").is_err());
        std::os::unix::fs::symlink(outside.path(), project.path().join("escape")).unwrap();
        assert!(inside_project(&root, "escape").is_err());
        assert!(inside_project(&root, "missing.app")
            .unwrap_err()
            .contains("no app"));
    }

    #[test]
    fn taps_by_label_only_what_it_can_tell_apart() {
        let (_, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        let plain = numbered(&elements);
        assert_eq!(labelled(&plain, "Settings"), Ok((340.0, 434.0)));
        assert_eq!(labelled(&plain, "safa"), Ok((58.0, 745.0)));
        assert!(labelled(&plain, "Camera")
            .unwrap_err()
            .contains("off the screen"));
        assert!(labelled(&plain, "Maps").unwrap_err().contains("no element"));
        let mut twice = elements;
        twice[1].label = "Settings Pro".into();
        let twice = numbered(&twice);
        assert_eq!(labelled(&twice, "Settings"), Ok((340.0, 434.0)));
        let ambiguous = labelled(&twice, "Sett").unwrap_err();
        assert!(
            ambiguous.contains("2 elements match") && ambiguous.contains("by its number"),
            "{ambiguous}"
        );
    }

    #[test]
    fn a_blank_launch_screen_is_not_a_settled_one() {
        let status_bar = json!({ "elements": [
            { "type": "Application", "label": " ", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 } },
            { "type": "StaticText", "label": "9:27 PM", "frame": { "x": 60, "y": 20, "width": 44, "height": 26 } },
            { "type": "GenericElement", "label": "100% battery power", "frame": { "x": 330, "y": 20, "width": 36, "height": 26 } },
        ]});
        assert!(launching(&elements_from(&status_bar, Some((402.0, 874.0)))));
        assert!(!launching(&elements_from(
            &home_screen(),
            Some((402.0, 874.0))
        )));
    }

    #[test]
    fn warns_when_a_swipe_starts_at_an_edge() {
        let screen = Some((402.0, 874.0));
        assert!(edge_warning(screen, (200.0, 873.0))
            .unwrap()
            .contains("bottom"));
        assert!(edge_warning(screen, (1.0, 400.0)).unwrap().contains("left"));
        assert_eq!(edge_warning(screen, (200.0, 600.0)), None);
    }

    /// Answers each request type the way sikemux-sim does, and records every request line.
    fn recording_helper(log: &std::path::Path) -> String {
        let tree = home_screen().to_string().replace('"', "\\\"");
        let tree = tree.get(1..tree.len() - 1).unwrap_or_default();
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U1\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"shutdown\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":402,\"height\":874,\"scale\":3}}" ;;
    *'"type":"tree"'*) echo "{{\"id\":$id,\"type\":\"result\",{tree}}}" ;;
    *'"generation":7'*'"type":"logs"'*) echo "{{\"id\":$id,\"type\":\"result\",\"lines\":[\"later\"],\"cursor\":9,\"more\":false,\"dropped\":0,\"reset\":false,\"generation\":7}}" ;;
    *'"after":5'*'"type":"logs"'*) echo "{{\"id\":$id,\"type\":\"result\",\"lines\":[\"first\"],\"cursor\":1,\"more\":false,\"dropped\":0,\"reset\":true,\"generation\":7}}" ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display()
        )
    }

    /// CI's fixture run failed `sim.tap` when a read caught the moment with no app in front.
    #[test]
    fn a_read_that_finds_no_app_in_front_looks_again() {
        let log_dir = tempfile::tempdir().unwrap();
        let no_app = log_dir.path().join("no-app");
        let script = recording_helper(&log_dir.path().join("requests")).replace(
            r#"*'"type":"tree"'*) echo"#,
            &format!(
                r#"*'"type":"tree"'*) [ -e '{}' ] && rm '{}' && echo "{{\"id\":$id,\"type\":\"error\",\"reason\":\"simulator\",\"message\":\"No translation object returned for simulator.\"}}" && continue; echo"#,
                no_app.display(),
                no_app.display()
            ),
        );
        let (_dir, manager) = helper(&script);
        run_now(
            &manager,
            "agent-1",
            "/tmp",
            "sim.attach",
            &json!({ "device": "iPhone 17" }),
        )
        .expect("attach");
        std::fs::write(&no_app, "").unwrap();
        run_now(
            &manager,
            "agent-1",
            "/tmp",
            "sim.tap",
            &json!({ "index": 0 }),
        )
        .expect("tap");
        assert!(!no_app.exists(), "the read that found no app was made");
    }

    #[test]
    fn drags_through_each_point_in_order_and_lets_go() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("requests");
        let (_dir, manager) = helper(&recording_helper(&log));
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-1", "/tmp", method, &params)
        };
        call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");

        call(
            "sim.touchPath",
            json!({ "points": [{ "x": 10, "y": 20 }, { "x": 30, "y": 40 }, { "x": 50, "y": 60 }], "duration": 0.1 }),
        )
        .expect("touch path");
        call(
            "sim.touch2Path",
            json!({ "points": [{ "x1": 100, "y1": 400, "x2": 300, "y2": 400 }, { "x1": 150, "y1": 400, "x2": 250, "y2": 400 }], "duration": 0.05 }),
        )
        .expect("pinch");
        assert!(
            call("sim.touchPath", json!({ "points": [{ "x": 1, "y": 2 }] }))
                .unwrap_err()
                .contains("two points")
        );
        assert!(call(
            "sim.touchPath",
            json!({ "points": [{ "x": 1 }, { "x": 2 }] })
        )
        .unwrap_err()
        .contains("x, y"));

        let paths: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|request| matches!(request["type"].as_str(), Some("touchPath" | "touch2Path")))
            .map(|request| json!([request["type"], request["points"]]))
            .collect();
        assert_eq!(
            paths,
            vec![
                json!(["touchPath", [
                    { "x": 10.0, "y": 20.0, "t": 0.0 },
                    { "x": 30.0, "y": 40.0, "t": 0.05 },
                    { "x": 50.0, "y": 60.0, "t": 0.1 },
                ]]),
                json!(["touch2Path", [
                    { "x": 100.0, "y": 400.0, "x2": 300.0, "y2": 400.0, "t": 0.0 },
                    { "x": 150.0, "y": 400.0, "x2": 250.0, "y2": 400.0, "t": 0.05 },
                ]]),
            ]
        );

        let turned = call("sim.rotate", json!({ "orientation": "landscapeLeft" })).expect("rotate");
        assert_eq!(
            turned["screen"],
            json!({ "width": 402.0, "height": 874.0 }),
            "the screen is the app's own size, as the read gives it"
        );
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains(r#""orientation":"landscapeLeft""#));

        let restarted = call("sim.logs", json!({ "cursor": 5 })).expect("logs");
        assert_eq!(restarted["lines"], json!(["first"]));
        assert!(restarted["restarted"]
            .as_str()
            .unwrap()
            .contains("started over"));
        assert_eq!(restarted["generation"], json!(7));
        let later = call("sim.logs", json!({ "cursor": 1 })).expect("logs");
        assert_eq!(
            later["lines"],
            json!(["later"]),
            "the generation is passed back"
        );
        assert!(later.get("restarted").is_none());

        let detached = call("sim.detach", json!({})).expect("detach");
        assert_eq!(detached["udid"], json!("U1"));
        assert!(call("sim.state", json!({}))
            .unwrap_err()
            .contains("sim attach"));
    }

    #[test]
    fn an_agent_attaches_reads_and_taps_by_number() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("requests");
        let (_dir, manager) = helper(&recording_helper(&log));
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-1", "/tmp", method, &params)
        };

        assert_eq!(
            call("sim.tap", json!({ "index": 0 })).unwrap_err(),
            "no simulator is attached; call sim attach first"
        );
        assert!(
            call("sim.install", json!({ "path": "/" })).is_err(),
            "nothing is attached yet"
        );
        let attached = call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");
        assert_eq!(attached["device"], json!("iPhone 17 (iOS 27.0)"));
        assert_eq!(attached["app"], json!("Home Screen"));
        assert!(attached["shown"]
            .as_str()
            .unwrap()
            .starts_with("live on your desk"));
        assert_eq!(
            attached["elements"][0],
            json!("0 Button \"Settings\" at (340, 434)")
        );

        assert_eq!(
            call("sim.tap", json!({ "index": 0 })).expect("tap")["changes"],
            json!("none")
        );
        let full = call("sim.tap", json!({ "index": 0, "report": "full" })).expect("tap");
        assert_eq!(
            full["elements"][0],
            json!("0 Button \"Settings\" at (340, 434)")
        );
        let outcome = call("sim.tap", json!({ "index": 0, "report": "outcome" })).expect("tap");
        assert!(
            outcome.get("elements").is_none() && outcome.get("changes").is_none(),
            "{outcome}"
        );
        assert!(call("sim.tap", json!({ "index": 0, "report": "everything" })).is_err());
        let devices = call("sim.devices", json!({})).expect("devices");
        assert_eq!(devices["devices"][0]["attached"], json!(true));
        assert_eq!(
            inspect(&manager, Some("agent-1"))["attached"],
            json!({ "udid": "U1", "name": "iPhone 17", "os": "iOS 27.0" })
        );
        assert_eq!(inspect(&manager, Some("agent-2"))["attached"], json!(null));
        let other_agent = run_now(&manager, "agent-2", "/tmp", "sim.state", &json!({}));
        assert!(other_agent.is_err(), "attachments belong to one agent");
        let taken = run_now(
            &manager,
            "agent-2",
            "/work/blog",
            "sim.attach",
            &json!({ "device": "iPhone 17" }),
        )
        .unwrap_err();
        assert!(
            taken.contains("in use by another agent in tmp") && taken.contains("sim devices"),
            "{taken}"
        );
        let listed = run_now(&manager, "agent-2", "/work/blog", "sim.devices", &json!({})).unwrap();
        assert_eq!(
            listed["devices"][0]["inUseBy"],
            json!("another agent in tmp")
        );

        let requests = std::fs::read_to_string(&log).unwrap();
        assert!(
            requests.contains(r#""type":"boot""#) && requests.contains(r#""udid":"U1""#),
            "{requests}"
        );
        let tap = requests
            .lines()
            .find(|line| line.contains(r#""type":"tap""#))
            .expect("a tap was sent");
        let tap: serde_json::Value = serde_json::from_str(tap).unwrap();
        assert_eq!(
            (tap["x"].as_f64(), tap["y"].as_f64()),
            (Some(340.0), Some(434.0))
        );
    }

    fn read_from(reply: &serde_json::Value) -> Read {
        let size = app_size(reply);
        let (app, elements, frames) = screen_from(reply, size);
        let mut flat = Vec::new();
        fn collect<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
            for element in value.as_array().into_iter().flatten() {
                out.push(element);
                collect(&element["children"], out);
            }
        }
        collect(&reply["elements"], &mut flat);
        let keyboard = flat.iter().any(|element| {
            element["traits"]
                .as_array()
                .is_some_and(|traits| traits.iter().any(|trait_| trait_ == "KeyboardKey"))
        });
        let text_fields = text_fields_from(&flat);
        Read {
            app,
            pid: frontmost_pid(reply),
            bundle_id: frontmost_bundle_id(reply),
            elements,
            frames,
            size,
            keyboard,
            text_fields,
        }
    }

    #[test]
    fn sim_type_dispatches_newlines_control_keys_and_submit_as_key_events() {
        assert_eq!(
            type_steps("mobile-e2e@swishx.com\n", false),
            vec![
                TypeStep::Text("mobile-e2e@swishx.com".into()),
                TypeStep::Key("Enter"),
            ]
        );
        assert_eq!(
            type_steps("mobile-e2e@swishx.com", true),
            vec![
                TypeStep::Text("mobile-e2e@swishx.com".into()),
                TypeStep::Key("Enter"),
            ]
        );
        assert_eq!(
            type_steps("a\r\nb\rc\td\u{8}", false),
            vec![
                TypeStep::Text("a".into()),
                TypeStep::Key("Enter"),
                TypeStep::Text("b".into()),
                TypeStep::Key("Enter"),
                TypeStep::Text("c".into()),
                TypeStep::Key("Tab"),
                TypeStep::Text("d".into()),
                TypeStep::Key("Backspace"),
            ]
        );
        let boundary = format!("{}\r\nz", "x".repeat(39));
        assert_eq!(
            type_steps(&boundary, false),
            vec![
                TypeStep::Text("x".repeat(39)),
                TypeStep::Key("Enter"),
                TypeStep::Text("z".into()),
            ],
            "CRLF at a 40-char chunk boundary still sends a single Enter"
        );
        assert_eq!(
            type_steps(r"mobile-e2e@swishx.com\n", false),
            vec![TypeStep::Text(r"mobile-e2e@swishx.com\n".into())],
            "literal backslash-n is not silently rewritten"
        );
        assert!(trailing_escaped_newline(r"mobile-e2e@swishx.com\n"));
        assert!(trailing_escaped_newline(r"\n"));
        assert!(!trailing_escaped_newline(r"path\\n"));
        assert!(!trailing_escaped_newline(r"C:\new\file"));

        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("requests");
        let value_file = log_dir.path().join("field-value");
        std::fs::write(&value_file, "").unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U1\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"booted\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":402,\"height\":874,\"scale\":3}}" ;;
    *'"type":"text"'*)
      val=$(printf '%s' "$line" | sed -E 's/.*"text":"([^"]*)".*/\1/')
      printf '%s' "$val" > '{value_file}'
      echo "{{\"id\":$id,\"type\":\"result\"}}"
      ;;
    *'"type":"tree"'*)
      val=$(cat '{value_file}')
      echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"App\",\"frame\":{{\"x\":0,\"y\":0,\"width\":402,\"height\":874}},\"children\":[{{\"type\":\"TextField\",\"label\":\"Email\",\"value\":\"$val\",\"enabled\":true,\"traits\":[\"IsEditing\"],\"frame\":{{\"x\":20,\"y\":200,\"width\":360,\"height\":44}}}}]}}]}}"
      ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display(),
            value_file = value_file.display(),
        );
        let (_dir, manager) = helper(&script);
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-type", "/tmp", method, &params)
        };
        call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");

        let ok = call("sim.type", json!({ "text": "mobile-e2e@swishx.com\n" })).expect("newline");
        assert!(ok.get("warning").is_none());

        let submitted = call(
            "sim.type",
            json!({ "text": "next@swishx.com", "submit": true }),
        )
        .expect("submit");
        assert!(submitted.get("warning").is_none());

        let escaped = call("sim.type", json!({ "text": r"literal@swishx.com\n" }))
            .expect("escaped newline still types");
        assert!(
            escaped["warning"]
                .as_str()
                .is_some_and(|w| w.contains("submit: true")),
            "{escaped}"
        );

        let sent: Vec<(String, serde_json::Value)> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|req| matches!(req["type"].as_str(), Some("text" | "key")))
            .filter(|req| req["key"].as_str() != Some("Shift"))
            .map(|req| {
                let kind = req["type"].as_str().unwrap().to_owned();
                let payload = if kind == "text" {
                    req["text"].clone()
                } else {
                    req["key"].clone()
                };
                (kind, payload)
            })
            .collect();
        assert_eq!(
            sent,
            vec![
                ("text".into(), json!("mobile-e2e@swishx.com")),
                ("key".into(), json!("Enter")),
                ("text".into(), json!("next@swishx.com")),
                ("key".into(), json!("Enter")),
                ("text".into(), json!(r"literal@swishx.com\n")),
            ]
        );
    }

    #[test]
    fn sim_type_accepts_text_matching_placeholder_when_focused_and_rejects_missed_focus() {
        let otp_screen = |value: &str,
                          traits: Vec<&str>,
                          button_enabled: bool,
                          clock: &str,
                          keyboard: bool| {
            let mut children = vec![
                json!({ "type": "StaticText", "label": clock, "frame": { "x": 60, "y": 20, "width": 50, "height": 20 } }),
                json!({ "type": "TextField", "label": "OTP", "value": value, "enabled": true, "traits": traits,
                        "frame": { "x": 20, "y": 240, "width": 360, "height": 48 } }),
                json!({ "type": "Button", "label": "Verify & Sign In", "enabled": button_enabled,
                        "frame": { "x": 20, "y": 320, "width": 360, "height": 48 } }),
            ];
            if keyboard {
                children.push(
                    json!({ "type": "Button", "label": "0", "traits": ["KeyboardKey"],
                        "frame": { "x": 150, "y": 780, "width": 80, "height": 44 } }),
                );
            }
            read_from(&json!({ "elements": [
                { "type": "Application", "label": "Academy", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
                  "children": children }
            ]}))
        };

        // Repro: OTP TextField reports placeholder "000000" as AXValue before typing and
        // real "000000" after typing, with IsEditing focused and Verify button enabled.
        let before_focused = otp_screen("000000", vec!["IsEditing"], false, "9:27", false);
        let after_enabled = otp_screen("000000", vec!["IsEditing"], true, "9:27", false);
        assert!(landed(
            false,
            &before_focused,
            &after_enabled,
            "000000",
            false
        ));

        // Even when no other button changes state, a focused field whose value equals the typed text counts as landed.
        assert!(landed(
            false,
            &before_focused,
            &before_focused,
            "000000",
            false
        ));

        // Submitting blurs the field (IsEditing only before) and still counts as landed.
        let after_blurred = otp_screen("000000", vec![], true, "9:27", false);
        assert!(landed(
            false,
            &before_focused,
            &after_blurred,
            "000000\n",
            true
        ));

        // Custom input without IsEditing: either an active iOS keyboard before Shift or an app UI change confirms delivery.
        let before_unmarked = otp_screen("000000", vec![], false, "9:27", false);
        assert!(landed(
            true,
            &before_unmarked,
            &before_unmarked,
            "000000",
            false
        ));
        assert!(landed(
            false,
            &before_unmarked,
            &after_blurred,
            "000000",
            false
        ));

        // Missed focus: field is unfocused, no keyboard was up, and nothing changed — MUST fail even if text matches placeholder.
        assert!(!landed(
            false,
            &before_unmarked,
            &before_unmarked,
            "000000",
            false
        ));

        // Missed focus with only a status-bar clock tick — MUST still fail.
        let after_clock_tick = otp_screen("000000", vec![], false, "9:28", false);
        assert!(!landed(
            false,
            &before_unmarked,
            &after_clock_tick,
            "000000",
            false
        ));

        // Focused field where keystrokes were dropped (typed "123456" but value stayed "000000") — MUST still fail.
        assert!(!landed(
            true,
            &before_focused,
            &before_focused,
            "123456",
            false
        ));

        // Two fields: Field 1 unfocused with placeholder "000000", Field 2 focused with "Email".
        // Dropping "000000" into Field 2 must NOT falsely match unfocused Field 1.
        let two_fields = read_from(&json!({ "elements": [
            { "type": "Application", "label": "Academy", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [
                { "type": "TextField", "label": "OTP", "value": "000000", "enabled": true, "traits": [],
                  "frame": { "x": 20, "y": 240, "width": 360, "height": 48 } },
                { "type": "TextField", "label": "Email", "value": "Email", "enabled": true, "traits": ["IsEditing"],
                  "frame": { "x": 20, "y": 160, "width": 360, "height": 48 } },
              ] }
        ]}));
        assert!(!landed(true, &two_fields, &two_fields, "000000", false));

        // Multiline React Native TextInput reports role "TextArea" and must be tracked as a text field.
        let text_area = read_from(&json!({ "elements": [
            { "type": "Application", "label": "Academy", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [
                { "type": "TextArea", "label": "Notes", "value": "", "enabled": true, "traits": ["TextArea", "TextEntry"],
                  "frame": { "x": 20, "y": 556, "width": 326, "height": 66 } },
              ] }
        ]}));
        assert_eq!(text_area.text_fields.len(), 1);
        assert!(!landed(false, &text_area, &text_area, "hello", false));

        // End-to-end sim.type repro: focused OTP field with placeholder "000000" succeeds when typing "000000",
        // while an unfocused field still fails with NOT_TYPED.
        let dir = tempfile::tempdir().unwrap();
        let focused_flag = dir.path().join("focused");
        std::fs::write(&focused_flag, "1").unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U1\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"booted\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":402,\"height\":874,\"scale\":3}}" ;;
    *'"type":"tree"'*)
      if [ -e '{flag}' ]; then
        traits='["IsEditing"]'
      else
        traits='[]'
      fi
      echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":402,\"height\":874}},\"children\":[{{\"type\":\"TextField\",\"label\":\"OTP\",\"value\":\"000000\",\"enabled\":true,\"traits\":$traits,\"frame\":{{\"x\":20,\"y\":240,\"width\":360,\"height\":48}}}}]}}]}}"
      ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            flag = focused_flag.display(),
        );
        let (_dir, manager) = helper(&script);
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-otp", "/tmp", method, &params)
        };
        call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");
        call("sim.type", json!({ "text": "000000" }))
            .expect("typing text equal to placeholder into a focused field succeeds");
        std::fs::remove_file(&focused_flag).unwrap();
        let missed = call("sim.type", json!({ "text": "000000" })).unwrap_err();
        assert!(missed.contains("no text field's value changed"), "{missed}");
    }

    #[test]
    fn sim_tap_auto_retaps_once_when_popover_dismiss_region_intercepts_target() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests");
        let mode = dir.path().join("mode");
        std::fs::write(&mode, "popover").unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"P1\",\"name\":\"iPad Pro 11-inch (M5)\",\"runtime\":\"iOS 27.0\",\"state\":\"booted\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":834,\"height\":1194,\"scale\":2}}" ;;
    *'"type":"tap"'*)
      cur=$(cat '{mode}')
      if [ "$cur" = "popover" ]; then
        printf 'open' > '{mode}'
      elif [ "$cur" = "open" ]; then
        printf 'signed_in' > '{mode}'
      elif [ "$cur" = "shifted_popover" ]; then
        printf 'shifted_open' > '{mode}'
      elif [ "$cur" = "nested_popover" ]; then
        printf 'popover' > '{mode}'
      fi
      echo "{{\"id\":$id,\"type\":\"result\"}}"
      ;;
    *'"type":"tree"'*)
      cur=$(cat '{mode}')
      if [ "$cur" = "popover" ] || [ "$cur" = "shifted_popover" ] || [ "$cur" = "nested_popover" ]; then
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"Button\",\"label\":\"Verify & Sign In\",\"enabled\":true,\"frame\":{{\"x\":200,\"y\":420,\"width\":400,\"height\":48}}}},{{\"type\":\"Button\",\"label\":\"Popover Action\",\"enabled\":true,\"frame\":{{\"x\":500,\"y\":700,\"width\":160,\"height\":44}}}},{{\"type\":\"Group\",\"label\":\"dismiss popup\",\"identifier\":\"PopoverDismissRegion\",\"enabled\":true,\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}}}}]}}]}}"
      elif [ "$cur" = "open" ]; then
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"Button\",\"label\":\"Verify & Sign In\",\"enabled\":true,\"frame\":{{\"x\":200,\"y\":420,\"width\":400,\"height\":48}}}}]}}]}}"
      elif [ "$cur" = "shifted_open" ]; then
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"Button\",\"label\":\"Verify & Sign In\",\"enabled\":true,\"frame\":{{\"x\":200,\"y\":620,\"width\":400,\"height\":48}}}}]}}]}}"
      else
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"StaticText\",\"label\":\"Signed In\",\"enabled\":true,\"frame\":{{\"x\":200,\"y\":200,\"width\":200,\"height\":32}}}}]}}]}}"
      fi
      ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display(),
            mode = mode.display(),
        );
        let (_dir, manager) = helper(&script);
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-ipad", "/tmp", method, &params)
        };
        let tap_count = || {
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|line| line.contains(r#""type":"tap""#))
                .count()
        };
        call("sim.attach", json!({ "device": "iPad Pro 11-inch (M5)" })).expect("attach");

        // 1. Tapping a target by label underneath PopoverDismissRegion auto-retaps once and activates it.
        let before_taps = tap_count();
        let retapped = call(
            "sim.tap",
            json!({ "label": "Verify & Sign In", "report": "full" }),
        )
        .expect("label tap");
        assert_eq!(tap_count() - before_taps, 2);
        assert!(
            retapped["note"]
                .as_str()
                .is_some_and(|n| n.contains("PopoverDismissRegion") && n.contains("tapped again")),
            "{retapped}"
        );
        assert!(
            retapped["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap().contains("Signed In")),
            "{retapped}"
        );

        // 2. Tapping a target by index underneath PopoverDismissRegion also auto-retaps once.
        std::fs::write(&mode, "popover").unwrap();
        let state = call("sim.state", json!({})).expect("state");
        let verify_index: u64 = state["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|line| {
                let s = line.as_str()?;
                s.contains("Verify & Sign In")
                    .then(|| s.split_whitespace().next()?.parse().ok())
                    .flatten()
            })
            .expect("verify button index");
        let before_taps = tap_count();
        let by_index = call("sim.tap", json!({ "index": verify_index })).expect("index tap");
        assert_eq!(tap_count() - before_taps, 2);
        assert!(by_index["note"]
            .as_str()
            .is_some_and(|n| n.contains("tapped again")));

        // 3. Point tap directly on an interactive target underneath auto-retaps once.
        std::fs::write(&mode, "popover").unwrap();
        let before_taps = tap_count();
        let by_point = call("sim.tap", json!({ "x": 400, "y": 444 })).expect("point tap on button");
        assert_eq!(tap_count() - before_taps, 2);
        assert!(by_point["note"]
            .as_str()
            .is_some_and(|n| n.contains("tapped again")));

        // 4. Point tap on empty space intentionally dismisses without double-tapping.
        std::fs::write(&mode, "popover").unwrap();
        let before_taps = tap_count();
        let empty = call("sim.tap", json!({ "x": 40, "y": 100 })).expect("point tap empty space");
        assert_eq!(tap_count() - before_taps, 1, "must not retap empty space");
        assert!(empty.get("note").is_none(), "{empty}");

        // 5. Explicitly targeting PopoverDismissRegion does not retap or warn.
        std::fs::write(&mode, "popover").unwrap();
        let before_taps = tap_count();
        let explicit = call("sim.tap", json!({ "label": "dismiss popup" })).expect("dismiss");
        assert_eq!(tap_count() - before_taps, 1);
        assert!(explicit.get("note").is_none(), "{explicit}");

        // 6. Tapping an action inside the popover that closes it (and disappears) does NOT retap.
        std::fs::write(&mode, "popover").unwrap();
        let before_taps = tap_count();
        let inside = call("sim.tap", json!({ "label": "Popover Action" })).expect("popover action");
        assert_eq!(tap_count() - before_taps, 1);
        assert!(inside.get("note").is_none(), "{inside}");

        // 7. Point tap where layout shifted away after popover dismissal does NOT retap the old point.
        std::fs::write(&mode, "shifted_popover").unwrap();
        let before_taps = tap_count();
        let shifted = call("sim.tap", json!({ "x": 400, "y": 444 })).expect("shifted point tap");
        assert_eq!(tap_count() - before_taps, 1);
        assert!(
            shifted["note"]
                .as_str()
                .is_some_and(|n| n.contains("tap again")),
            "{shifted}"
        );

        // 8. Nested popover stays bounded (no retap when PopoverDismissRegion is still present).
        std::fs::write(&mode, "nested_popover").unwrap();
        let before_taps = tap_count();
        let _nested = call("sim.tap", json!({ "label": "Verify & Sign In" })).expect("nested");
        assert_eq!(tap_count() - before_taps, 1);
    }

    #[test]
    fn sim_type_warns_when_readback_misses_a_dropped_character() {
        let field_screen = |role: &str, value: &str| {
            read_from(&json!({ "elements": [
                { "type": "Application", "label": "Academy", "frame": { "x": 0, "y": 0, "width": 834, "height": 1194 },
                  "children": [
                    { "type": role, "label": "Slug", "value": value, "enabled": true, "traits": ["IsEditing"],
                      "frame": { "x": 40, "y": 240, "width": 400, "height": 48 } }
                  ] }
            ]}))
        };

        let empty = field_screen("TextField", "");
        let dropped = field_screen("TextField", "dem-owner");
        let warning = dropped_text_warning(&empty, &dropped, "demo-owner", false)
            .expect("should warn when 'o' was dropped");
        assert!(
            warning.contains("demo-owner") && warning.contains("dem-owner"),
            "{warning}"
        );

        // Exact match, case fold, non-alphanumeric stripping, formatting, and SecureTextField do not warn.
        assert_eq!(
            dropped_text_warning(
                &empty,
                &field_screen("TextField", "demo-owner"),
                "demo-owner",
                false
            ),
            None
        );
        assert_eq!(
            dropped_text_warning(
                &empty,
                &field_screen("TextField", "demo-owner"),
                "Demo-Owner",
                false
            ),
            None
        );
        assert_eq!(
            dropped_text_warning(
                &empty,
                &field_screen("TextField", "5551234"),
                "555-1234",
                false
            ),
            None
        );
        assert_eq!(
            dropped_text_warning(
                &empty,
                &field_screen("TextField", "(555) 123-4"),
                "5551234",
                false
            ),
            None
        );
        assert_eq!(
            dropped_text_warning(
                &empty,
                &field_screen("SecureTextField", "•••••"),
                "secret",
                false
            ),
            None
        );
        assert_eq!(
            dropped_text_warning(&empty, &dropped, "demo-owner", true),
            None
        );

        // Appending into an existing value detects a drop in the newly appended suffix.
        let prefixed = field_screen("TextField", "team-");
        let appended_drop = field_screen("TextField", "team-dem-owner");
        assert!(dropped_text_warning(&prefixed, &appended_drop, "demo-owner", false).is_some());
    }

    #[test]
    fn sim_open_url_errors_when_no_app_handles_custom_scheme_and_allows_system_schemes() {
        assert_eq!(
            custom_url_scheme("MyApp://expo-development-client/?url=http://127.0.0.1:8081"),
            Some("myapp".into())
        );
        for system in [
            "http://127.0.0.1:8081",
            "https://example.com/invite",
            "tel:5551234",
            "mailto:test@example.com",
            "prefs:root=WIFI",
            "maps://?q=apple",
        ] {
            assert_eq!(custom_url_scheme(system), None, "{system}");
        }

        let url_types = json!([{ "CFBundleURLSchemes": ["SwishXAcademy", "com.swishx.academy"] }]);
        assert!(url_types_handle_scheme(&url_types, "swishxacademy"));
        assert!(!url_types_handle_scheme(&url_types, "otherapp"));

        let springboard = read_from(&home_screen());
        let system_blank = read_from(&json!({ "elements": [
            { "type": "Application", "label": " ", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [{ "type": "StaticText", "label": "9:41", "frame": { "x": 40, "y": 20, "width": 44, "height": 20 } }] }
        ]}));
        let system_alert = read_from(&json!({ "elements": [
            { "type": "Application", "label": " ", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [{ "type": "Button", "label": "Open", "frame": { "x": 200, "y": 460, "width": 120, "height": 44 } }] }
        ]}));
        let opened_app = read_from(&json!({ "elements": [
            { "type": "Application", "label": "Academy", "frame": { "x": 0, "y": 0, "width": 402, "height": 874 },
              "children": [{ "type": "StaticText", "label": "Welcome", "frame": { "x": 40, "y": 200, "width": 200, "height": 32 } }] }
        ]}));

        let err = unhandled_custom_url_scheme_error(
            "myapp://open",
            &springboard,
            &springboard,
            Some(false),
        )
        .expect("unhandled custom scheme on Home Screen must error");
        assert!(
            err.contains("myapp://") && err.contains("sim install"),
            "{err}"
        );

        assert!(unhandled_custom_url_scheme_error(
            "myapp://open",
            &system_blank,
            &system_blank,
            None,
        )
        .is_some());
        assert_eq!(
            unhandled_custom_url_scheme_error("myapp://open", &springboard, &opened_app, None),
            None
        );
        assert_eq!(
            unhandled_custom_url_scheme_error("myapp://open", &springboard, &system_alert, None),
            None
        );
        assert_eq!(
            unhandled_custom_url_scheme_error(
                "https://example.com",
                &springboard,
                &springboard,
                Some(false)
            ),
            None
        );

        // End-to-end sim.openUrl through the fake helper: stays on Home Screen -> error for custom scheme, ok for https.
        let dir = tempfile::tempdir().unwrap();
        let (_helper_dir, manager) = helper(&recording_helper(&dir.path().join("requests")));
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-url", "/tmp", method, &params)
        };
        call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");
        let missing = call("sim.openUrl", json!({ "url": "swishxacademy://open" })).unwrap_err();
        assert!(
            missing.contains("no installed app handles scheme 'swishxacademy://'")
                && missing.contains("sim install"),
            "{missing}"
        );
        call("sim.openUrl", json!({ "url": "https://example.com" }))
            .expect("built-in https scheme must not false-positive");
    }

    #[test]
    fn duplicate_custom_url_scheme_warns_on_open_and_install_and_uninstall_removes_bundle() {
        let conflicts = duplicate_scheme_conflicts_from(
            &[
                (
                    "com.swishx.salesacademy".into(),
                    vec!["swishxacademy".into()],
                ),
                ("com.swishx.academy".into(), vec!["swishxacademy".into()]),
                ("com.example.other".into(), vec!["other".into()]),
            ],
            "com.swishx.academy",
            &["swishxacademy".into()],
        );
        assert_eq!(
            conflicts,
            vec![(
                "swishxacademy".into(),
                vec![
                    "com.swishx.academy".into(),
                    "com.swishx.salesacademy".into()
                ]
            )]
        );
        let install_warn =
            duplicate_scheme_install_warning(&conflicts).expect("install should warn");
        assert!(
            install_warn.contains("swishxacademy://")
                && install_warn.contains("com.swishx.academy")
                && install_warn.contains("com.swishx.salesacademy")
                && install_warn.contains("sim uninstall"),
            "{install_warn}"
        );
        assert_eq!(duplicate_scheme_install_warning(&[]), None);

        let open_warn = duplicate_scheme_open_warning(
            "swishxacademy",
            &[
                "com.swishx.academy".into(),
                "com.swishx.salesacademy".into(),
            ],
            Some("com.swishx.salesacademy"),
            "Sales Academy",
        )
        .expect("open should warn");
        assert!(
            open_warn.contains("swishxacademy://")
                && open_warn.contains("com.swishx.academy")
                && open_warn.contains("com.swishx.salesacademy")
                && open_warn.contains("opened com.swishx.salesacademy in the foreground")
                && open_warn.contains("sim uninstall"),
            "{open_warn}"
        );
        assert_eq!(
            app_bundle_ancestor(std::path::Path::new(
                "/Devices/U1/data/Containers/Bundle/Application/UUID/Academy.app/Academy"
            )),
            Some(std::path::Path::new(
                "/Devices/U1/data/Containers/Bundle/Application/UUID/Academy.app"
            ))
        );

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests");
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U1\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"booted\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":402,\"height\":874,\"scale\":3}}" ;;
    *'"type":"openUrl"'*) echo "{{\"id\":$id,\"type\":\"result\",\"scheme\":\"swishxacademy\",\"matchingBundleIds\":[\"com.swishx.academy\",\"com.swishx.salesacademy\"]}}" ;;
    *'"type":"tree"'*) echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Sales Academy\",\"bundleId\":\"com.swishx.salesacademy\",\"pid\":999,\"frame\":{{\"x\":0,\"y\":0,\"width\":402,\"height\":874}},\"children\":[{{\"type\":\"StaticText\",\"label\":\"Welcome\",\"frame\":{{\"x\":40,\"y\":200,\"width\":200,\"height\":32}}}}]}}]}}" ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display(),
        );
        let (_helper_dir, manager) = helper(&script);
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-dup", "/tmp", method, &params)
        };
        call("sim.attach", json!({ "device": "iPhone 17" })).expect("attach");
        let opened = call(
            "sim.openUrl",
            json!({ "url": "swishxacademy://expo-development-client/?url=http%3A%2F%2F127.0.0.1%3A8081" }),
        )
        .expect("openUrl with duplicate scheme succeeds with warning");
        let warning = opened["warning"].as_str().expect("duplicate warning");
        assert!(
            warning.contains("com.swishx.academy")
                && warning.contains("com.swishx.salesacademy")
                && warning.contains("opened com.swishx.salesacademy in the foreground")
                && warning.contains("sim uninstall"),
            "{opened}"
        );

        let removed = call(
            "sim.uninstall",
            json!({ "bundleId": "com.swishx.salesacademy" }),
        )
        .expect("uninstall");
        assert_eq!(removed["uninstalled"], json!("com.swishx.salesacademy"));
        let sent = std::fs::read_to_string(&log).unwrap();
        assert!(
            sent.contains(r#""type":"uninstall""#)
                && sent.contains(r#""bundleId":"com.swishx.salesacademy""#),
            "{sent}"
        );
    }

    #[test]
    fn sim_attach_reclaims_idle_booted_simulator_and_reports_slot_holders() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests");
        let iphone_state = dir.path().join("iphone-state");
        let ipad_state = dir.path().join("ipad-state");
        std::fs::write(&iphone_state, "booted").unwrap();
        std::fs::write(&ipad_state, "shutdown").unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*)
      ip=$(cat '{iphone}')
      pd=$(cat '{ipad}')
      echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U-IPHONE\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"$ip\"}},{{\"udid\":\"U-IPAD\",\"name\":\"iPad Pro 11-inch (M5)\",\"runtime\":\"iOS 27.0\",\"state\":\"$pd\"}}]}}"
      ;;
    *'"type":"shutdown"'*)
      case "$line" in
        *'"udid":"U-IPHONE"'*) printf 'shutdown' > '{iphone}' ;;
        *'"udid":"U-IPAD"'*) printf 'shutdown' > '{ipad}' ;;
      esac
      echo "{{\"id\":$id,\"type\":\"result\"}}"
      ;;
    *'"type":"boot"'*)
      case "$line" in
        *'"udid":"U-IPHONE"'*) printf 'booted' > '{iphone}' ;;
        *'"udid":"U-IPAD"'*) printf 'booted' > '{ipad}' ;;
      esac
      echo "{{\"id\":$id,\"type\":\"result\"}}"
      ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":834,\"height\":1194,\"scale\":2}}" ;;
    *'"type":"tree"'*) echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"StaticText\",\"label\":\"Home\",\"frame\":{{\"x\":40,\"y\":200,\"width\":200,\"height\":32}}}}]}}]}}" ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display(),
            iphone = iphone_state.display(),
            ipad = ipad_state.display(),
        );
        let (_helper_dir, manager) = helper(&script);
        let busy_machine = Machine {
            cores: 10,
            free_memory: Some(8 << 30),
            load: Some(20.0),
        };
        manager.set_test_machine(busy_machine, false);
        manager.note_booted("U-IPHONE");

        // 1. Idle unattached iPhone 17 blocks slot under high load -> reclaimed immediately so iPad boots.
        let attached = run_now(
            &manager,
            "agent-ino",
            "/work/academy-mobile",
            "sim.attach",
            &json!({ "device": "iPad Pro 11-inch (M5)", "fresh": true }),
        )
        .expect("should reclaim idle iPhone 17 and boot iPad");
        assert!(
            attached["device"]
                .as_str()
                .is_some_and(|d| d.contains("iPad Pro 11-inch (M5)")),
            "{attached}"
        );
        assert_eq!(std::fs::read_to_string(&iphone_state).unwrap(), "shutdown");
        assert_eq!(std::fs::read_to_string(&ipad_state).unwrap(), "booted");

        // 2. Switching from own attached iPhone 17 to iPad Pro also reclaims own iPhone 17 cleanly.
        manager.release("agent-ino");
        std::fs::write(&iphone_state, "booted").unwrap();
        std::fs::write(&ipad_state, "shutdown").unwrap();
        manager.note_booted("U-IPHONE");
        run_now(
            &manager,
            "agent-ino",
            "/work/academy-mobile",
            "sim.attach",
            &json!({ "device": "iPhone 17" }),
        )
        .expect("attach iphone");
        run_now(
            &manager,
            "agent-ino",
            "/work/academy-mobile",
            "sim.attach",
            &json!({ "device": "iPad Pro 11-inch (M5)", "fresh": true }),
        )
        .expect("switching devices reclaims own previous simulator");
        assert_eq!(std::fs::read_to_string(&iphone_state).unwrap(), "shutdown");

        // 3. When another active agent holds the slot, room() reports the exact device & agent.
        manager.release("agent-ino");
        std::fs::write(&iphone_state, "booted").unwrap();
        std::fs::write(&ipad_state, "shutdown").unwrap();
        run_now(
            &manager,
            "agent-other",
            "/work/academy-mobile",
            "sim.attach",
            &json!({ "device": "iPhone 17" }),
        )
        .expect("other agent attaches iphone");
        let devices = [
            Device {
                udid: "U-IPHONE".into(),
                name: "iPhone 17".into(),
                os: "iOS 27.0".into(),
                booted: true,
                busy: false,
                warm: true,
                screen: None,
            },
            Device {
                udid: "U-IPAD".into(),
                name: "iPad Pro 11-inch (M5)".into(),
                os: "iOS 27.0".into(),
                booted: false,
                busy: false,
                warm: true,
                screen: None,
            },
        ];
        let queued = manager
            .room("agent-ino", &devices, false, Some("U-IPAD"))
            .err()
            .expect("slot held by active agent should wait");
        let wait_msg = queued_text(&queued);
        assert!(
            wait_msg.contains("iPhone 17")
                && wait_msg.contains("agent-other")
                && wait_msg.contains("academy-mobile"),
            "{wait_msg}"
        );
        let too_long_msg = queued_text(&Queued::TooLong {
            wait: Some(Wait::Full {
                running: 1,
                limit: 1,
            }),
            machine: busy_machine,
            slots: vec!["iPhone 17 (held by agent-other in academy-mobile)".into()],
        });
        assert!(
            too_long_msg.contains("iPhone 17")
                && too_long_msg.contains("agent-other")
                && too_long_msg.contains("academy-mobile"),
            "{too_long_msg}"
        );

        // 4. External simulator open in Simulator.app at hard memory/core cap fails fast with clear message.
        manager.release("agent-other");
        manager.forget_booted("U-IPHONE");
        manager.set_test_machine(
            Machine {
                cores: 3,
                free_memory: Some(1 << 30),
                load: Some(1.0),
            },
            true,
        );
        let blocked = run_now(
            &manager,
            "agent-ino",
            "/work/academy-mobile",
            "sim.attach",
            &json!({ "device": "iPad Pro 11-inch (M5)", "fresh": true }),
        )
        .unwrap_err();
        assert!(
            blocked.contains("no simulator slot is available")
                && blocked.contains("iPhone 17")
                && blocked.contains("booted outside Sikemux"),
            "{blocked}"
        );
    }

    #[test]
    fn sim_tap_waits_out_transient_empty_tree_and_bounds_genuinely_empty_screens() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("requests");
        let mode = dir.path().join("mode");
        let count = dir.path().join("empty-count");
        std::fs::write(&mode, "modal").unwrap();
        std::fs::write(&count, "0").unwrap();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"P1\",\"name\":\"iPad Pro 11-inch (M5)\",\"runtime\":\"iOS 27.0\",\"state\":\"booted\"}}]}}" ;;
    *'"type":"screen"'*) echo "{{\"id\":$id,\"type\":\"result\",\"width\":834,\"height\":1194,\"scale\":2}}" ;;
    *'"type":"tap"'*)
      cur=$(cat '{mode}')
      if [ "$cur" = "modal" ]; then
        printf 'fade' > '{mode}'
        printf '0' > '{count}'
      elif [ "$cur" = "home" ]; then
        printf 'blank' > '{mode}'
      fi
      echo "{{\"id\":$id,\"type\":\"result\"}}"
      ;;
    *'"type":"tree"'*)
      cur=$(cat '{mode}')
      if [ "$cur" = "modal" ]; then
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"Button\",\"label\":\"Continue Watching\",\"enabled\":true,\"frame\":{{\"x\":200,\"y\":500,\"width\":400,\"height\":48}}}}]}}]}}"
      elif [ "$cur" = "fade" ]; then
        n=$(cat '{count}')
        n=$((n + 1))
        printf '%s' "$n" > '{count}'
        if [ "$n" -le 3 ]; then
          echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[]}}]}}"
        else
          printf 'home' > '{mode}'
          echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"StaticText\",\"label\":\"Course Home\",\"enabled\":true,\"frame\":{{\"x\":40,\"y\":120,\"width\":300,\"height\":32}}}},{{\"type\":\"Button\",\"label\":\"Open Canvas\",\"enabled\":true,\"frame\":{{\"x\":40,\"y\":220,\"width\":300,\"height\":48}}}}]}}]}}"
        fi
      elif [ "$cur" = "home" ]; then
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[{{\"type\":\"StaticText\",\"label\":\"Course Home\",\"enabled\":true,\"frame\":{{\"x\":40,\"y\":120,\"width\":300,\"height\":32}}}},{{\"type\":\"Button\",\"label\":\"Open Canvas\",\"enabled\":true,\"frame\":{{\"x\":40,\"y\":220,\"width\":300,\"height\":48}}}}]}}]}}"
      else
        echo "{{\"id\":$id,\"type\":\"result\",\"elements\":[{{\"type\":\"Application\",\"label\":\"Academy\",\"frame\":{{\"x\":0,\"y\":0,\"width\":834,\"height\":1194}},\"children\":[]}}]}}"
      fi
      ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display(),
            mode = mode.display(),
            count = count.display(),
        );
        let (_dir, manager) = helper(&script);
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-fade", "/tmp", method, &params)
        };
        let tree_reads = || {
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|line| line.contains(r#""type":"tree""#))
                .count()
        };

        call("sim.attach", json!({ "device": "iPad Pro 11-inch (M5)" })).expect("attach");

        // Dismissing a fading modal returns 3 transient empty trees before the underlying screen appears.
        let dismissed = call(
            "sim.tap",
            json!({ "label": "Continue Watching", "report": "full" }),
        )
        .expect("dismiss modal");
        let lines: Vec<&str> = dismissed["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap())
            .collect();
        assert!(
            lines.iter().any(|line| line.contains("Course Home")),
            "transient empty fade should settle onto the underlying screen: {dismissed}"
        );

        // Transitioning to a genuinely empty screen stops after a bounded number of empty retries.
        let before_blank = tree_reads();
        let blank = call(
            "sim.tap",
            json!({ "label": "Open Canvas", "report": "full" }),
        )
        .expect("open blank screen");
        assert_eq!(blank["elements"], json!([]));
        let blank_reads = tree_reads() - before_blank;
        // 1 pre-tap read + 1 initial post-tap read + 5 bounded empty retries = 7 tree reads.
        assert_eq!(blank_reads, 7, "genuinely empty screen must stop promptly");
    }

    /// Drives the fixture app the way an agent would, through every kind of tool:
    /// `pnpm build:sim-fixture`, then
    /// `SIKEMUX_SIM_EXECUTABLE=… SIKEMUX_SIM_FIXTURE=…/SimFixture.app cargo test --lib simulator -- --ignored`.
    #[test]
    #[ignore = "needs the sikemux-sim helper, Xcode's simulators and the fixture app"]
    fn an_agent_drives_the_fixture_app() {
        let _turn = REAL_DEVICE.locked();
        let fixture = std::env::var("SIKEMUX_SIM_FIXTURE")
            .expect("SIKEMUX_SIM_FIXTURE names the built SimFixture.app");
        let (_errors, manager) = real_helper();
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-fixture", "/", method, &params)
                .unwrap_or_else(|error| panic!("{method}: {error}"))
        };
        let shows = |state: &serde_json::Value, text: &str| {
            let lines = state["elements"]
                .as_array()
                .or(state["changes"]["elements"].as_array());
            lines
                .into_iter()
                .flatten()
                .any(|line| line.as_str().unwrap().contains(text))
        };
        attach_when_booted(&manager, "agent-fixture");
        call("sim.install", json!({ "path": fixture }));

        let opened = call(
            "sim.launch",
            json!({ "bundleId": "com.nodelike.sikemux.simfixture" }),
        );
        assert_eq!(opened["app"], json!("SimFixture"), "{opened}");
        assert!(shows(&opened, "Count: 0"), "{opened}");

        let tapped = call("sim.tap", json!({ "label": "Add one" }));
        assert!(
            shows(&tapped, "Count: 1"),
            "{tapped}, then {} and the app logged {}",
            call("sim.state", json!({})),
            call("sim.logs", json!({ "process": "SimFixture" }))["lines"]
        );

        call("sim.tap", json!({ "label": "field" }));
        let typed = call("sim.type", json!({ "text": "Hi there 42" }));
        assert!(shows(&typed, "Echo: Hi there 42"), "{typed}");

        let zoom = call("sim.state", json!({}))["elements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line.as_str().unwrap())
            .find(|line| line.contains("Zoom: 1.0"))
            .expect("the zoom view")
            .to_owned();
        let centre: Vec<f64> = zoom
            .rsplit_once(" at (")
            .unwrap()
            .1
            .trim_end_matches(')')
            .split(", ")
            .map(|n| n.parse().unwrap())
            .collect();
        let pinched = call(
            "sim.touch2Path",
            json!({ "points": [
                { "x1": centre[0] - 20.0, "y1": centre[1], "x2": centre[0] + 20.0, "y2": centre[1] },
                { "x1": centre[0] - 120.0, "y1": centre[1], "x2": centre[0] + 120.0, "y2": centre[1] },
            ], "duration": 0.5 }),
        );
        assert!(
            !shows(&pinched, "Zoom: 1.0"),
            "spreading two fingers zooms in: {pinched}"
        );

        call("sim.type", json!({ "text": "\n" }));
        let scrolled = call(
            "sim.swipe",
            json!({ "fromX": 200, "fromY": 780, "toX": 200, "toY": 520, "duration": 0.4 }),
        );
        assert!(
            shows(&scrolled, "Row 1") || shows(&scrolled, "Row 2"),
            "the list is on screen: {scrolled}"
        );

        let logged = call("sim.logs", json!({ "process": "SimFixture" }));
        let lines = logged["lines"].as_array().unwrap();
        assert!(
            lines
                .iter()
                .any(|line| line.as_str().unwrap().contains("tapped add one, count 1")),
            "{logged}"
        );

        let turned = call("sim.rotate", json!({ "orientation": "landscapeLeft" }));
        println!("turned: {turned}");
        let tapped = call("sim.tap", json!({ "label": "Add one" }));
        assert!(
            shows(&tapped, "Count: 2"),
            "a turned app is tapped by label: {tapped}"
        );
        call("sim.rotate", json!({ "orientation": "portrait" }));

        call(
            "sim.terminate",
            json!({ "bundleId": "com.nodelike.sikemux.simfixture" }),
        );
        call("sim.detach", json!({}));
    }

    /// An agent's whole round on a real simulator:
    /// `SIKEMUX_SIM_EXECUTABLE=… cargo test --lib simulator -- --ignored`.
    #[test]
    #[ignore = "needs the sikemux-sim helper and Xcode's simulators"]
    fn an_agent_drives_a_real_simulator() {
        let _turn = REAL_DEVICE.locked();
        let (_errors, manager) = real_helper();
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-real", "/tmp", method, &params)
                .unwrap_or_else(|error| panic!("{method}: {error}"))
        };
        let attached = attach_when_booted(&manager, "agent-real");
        println!("attached: {}", attached["device"]);
        call("sim.button", json!({ "button": "home" }));
        let home = call("sim.button", json!({ "button": "home" }));
        assert_eq!(home["app"], json!("Home Screen"), "{home}");
        let settings = call("sim.tap", json!({ "label": "Settings" }));
        assert_eq!(settings["app"], json!("Settings"), "{settings}");
        println!(
            "{}",
            settings["elements"]
                .as_array()
                .unwrap()
                .iter()
                .take(5)
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let shot = call("sim.screenshot", json!({}));
        assert_eq!(shot["mimeType"], json!("image/jpeg"));
        assert!(shot["data"].as_str().unwrap().len() > 1000);
        let back = call("sim.button", json!({ "button": "home" }));
        assert_eq!(back["app"], json!("Home Screen"));
    }
}
