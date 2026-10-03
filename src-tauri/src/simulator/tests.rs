use std::os::unix::fs::PermissionsExt;

use tempfile::TempDir;

use super::SimulatorManager;

/// Tests that drive a real simulator share it, so they take turns.
static REAL_DEVICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn real_device() -> std::sync::MutexGuard<'static, ()> {
    REAL_DEVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn helper(script: &str) -> (TempDir, SimulatorManager) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("sikemux-sim");
    std::fs::write(&path, script).expect("write helper");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (dir, SimulatorManager::with_helper(path))
}

/// The helper built beside the tests, as `SIKEMUX_SIM_EXECUTABLE` names it.
fn real_helper() -> SimulatorManager {
    SimulatorManager::with_helper(
        crate::sim::local_helper().expect("SIKEMUX_SIM_EXECUTABLE names the sikemux-sim helper"),
    )
}

#[test]
#[ignore = "needs the sikemux-sim helper and Xcode's simulators"]
fn lists_the_real_simulators() {
    let _turn = real_device();
    let manager = real_helper();
    let devices = tauri::async_runtime::block_on(manager.request("devices", serde_json::json!({})))
        .expect("devices");
    let devices = devices["devices"].as_array().expect("a list of devices");
    assert!(!devices.is_empty(), "Xcode has no simulators");
    assert!(devices
        .iter()
        .all(|device| device["udid"].is_string() && device["state"].is_string()));
}

mod tools {
    use serde_json::json;

    use super::{helper, real_helper};
    use crate::simulator::tools::{
        changes, choose_device, edge_warning, element_lines, elements_from, inspect, labelled,
        launching, lines_of, run, tap_point, Device,
    };

    /// Runs one tool call to its end, as the harness does on its own thread.
    fn run_now(
        manager: &crate::simulator::SimulatorManager,
        agent_id: &str,
        project: &str,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        tauri::async_runtime::block_on(run(manager, agent_id, project, method, params))
    }

    fn device(name: &str, os: &str, booted: bool) -> Device {
        Device {
            udid: format!("{name}-{os}"),
            name: name.into(),
            os: os.into(),
            booted,
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
        assert_eq!(
            choose_device(&devices, Some("iPhone 17")).unwrap().os,
            "iOS 27.0"
        );
        assert_eq!(
            choose_device(&devices, Some("iPhone 17-iOS 26.0"))
                .unwrap()
                .os,
            "iOS 26.0"
        );
        let booted = [
            device("iPhone 17", "iOS 27.0", false),
            device("iPhone 16e", "iOS 26.0", true),
        ];
        assert_eq!(choose_device(&booted, None).unwrap().name, "iPhone 16e");
        assert_eq!(
            choose_device(&devices, None).unwrap().udid,
            "iPhone 17-iOS 27.0"
        );
        assert!(choose_device(&devices, Some("Pixel 9"))
            .unwrap_err()
            .contains("sim_devices"));
        assert!(choose_device(&[device("iPad Air", "iOS 27.0", true)], None).is_err());
    }

    #[test]
    fn numbers_the_elements_a_person_could_act_on() {
        let (app, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        assert_eq!(app, "Home Screen");
        assert_eq!(
            element_lines(&elements),
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
        assert_eq!(tap_point(&elements, Some(1), None, None), Ok((58.0, 745.0)));
        assert_eq!(
            tap_point(&elements, None, Some(3.0), Some(4.0)),
            Ok((3.0, 4.0))
        );
        assert!(tap_point(&elements, Some(9), None, None)
            .unwrap_err()
            .contains("sim_state"));
        assert!(tap_point(&elements, None, Some(3.0), None).is_err());
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
            element_lines(&elements),
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
            element_lines(&elements),
            vec![
                "0 TextField \"Address\" value=\"example.com\" at (200, 816)",
                "1 StaticText \"Say \\\"hi\\\" \\\\ bye\" at (50, 110)",
            ]
        );
    }

    #[test]
    fn tells_what_appeared_changed_and_went_away() {
        let (_, before) = elements_from(&home_screen(), Some((402.0, 874.0)));
        assert_eq!(changes(&before, &before), (vec![], vec![]));

        let mut after = before.clone();
        after[2].value = "dogs".into();
        after.remove(1);
        after.push(crate::simulator::tools::Element {
            role: "Button".into(),
            label: "Done".into(),
            value: String::new(),
            identifier: String::new(),
            enabled: true,
            center: (50.0, 50.0),
            offscreen: false,
        });
        let (changed, removed) = changes(&before, &after);
        assert_eq!(
            changed,
            vec![
                "1 TextField \"Search\" value=\"dogs\" [disabled] at (60, 25)",
                "3 Button \"Done\" at (50, 50)",
            ]
        );
        assert_eq!(removed, vec!["Button \"Safari\" at (58, 745)"]);
    }

    #[test]
    fn taps_by_label_only_what_it_can_tell_apart() {
        let (_, elements) = elements_from(&home_screen(), Some((402.0, 874.0)));
        assert_eq!(labelled(&elements, "Settings"), Ok((340.0, 434.0)));
        assert_eq!(labelled(&elements, "safa"), Ok((58.0, 745.0)));
        assert!(labelled(&elements, "Camera")
            .unwrap_err()
            .contains("off the screen"));
        assert!(labelled(&elements, "Maps")
            .unwrap_err()
            .contains("no element"));
        let mut twice = elements.clone();
        twice[1].label = "Settings Pro".into();
        assert_eq!(labelled(&twice, "Settings"), Ok((340.0, 434.0)));
        assert!(labelled(&twice, "Sett")
            .unwrap_err()
            .contains("2 elements match"));
    }

    #[test]
    fn reads_one_process_from_the_device_log_and_where_to_read_on() {
        let read = json!({
            "lines": [
                "2026-10-03 14:00:00.1 Df SimFixture[10:1] tapped",
                "2026-10-03 14:00:00.2 Df Maps[11:1] moved",
                "2026-10-03 14:00:00.3 Df SimFixture[10:1] typed",
                "2026-10-03 14:00:00.4 Df SimFixture[10:1] scrolled",
            ],
            "cursor": 14,
            "more": false,
        });
        let first = lines_of(&read, 10, "SimFixture", 2);
        assert_eq!(first["lines"].as_array().unwrap().len(), 2);
        assert_eq!(first["cursor"], json!(13));
        assert_eq!(first["more"], json!(true));
        let all = lines_of(&read, 10, "SimFixture", 10);
        assert_eq!(all["cursor"], json!(14));
        assert_eq!(all["more"], json!(false));
        assert_eq!(lines_of(&read, 10, "Notes", 10)["lines"], json!([]));
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
        let tree = &tree[1..tree.len() - 1];
        format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
  case "$line" in
    *'"type":"devices"'*) echo "{{\"id\":$id,\"type\":\"result\",\"devices\":[{{\"udid\":\"U1\",\"name\":\"iPhone 17\",\"runtime\":\"iOS 27.0\",\"state\":\"shutdown\",\"screen\":{{\"width\":402,\"height\":874}}}}]}}" ;;
    *'"type":"tree"'*) echo "{{\"id\":$id,\"type\":\"result\",{tree}}}" ;;
    *) echo "{{\"id\":$id,\"type\":\"result\"}}" ;;
  esac
done
"#,
            log = log.display()
        )
    }

    #[test]
    fn drags_through_each_point_in_order_and_lets_go() {
        let log_dir = tempfile::tempdir().unwrap();
        let log = log_dir.path().join("requests");
        let (_dir, manager) = helper(&recording_helper(&log));
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-1", "/tmp", method, &params)
        };
        call("sim.attach", json!({})).expect("attach");

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
        assert_eq!(turned["screen"], json!({ "width": 874.0, "height": 402.0 }));
        let upright =
            call("sim.rotate", json!({ "orientation": "portrait" })).expect("rotate back");
        assert_eq!(
            upright["screen"],
            json!({ "width": 402.0, "height": 874.0 })
        );
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains(r#""orientation":"landscapeLeft""#));

        let detached = call("sim.detach", json!({})).expect("detach");
        assert_eq!(detached["udid"], json!("U1"));
        assert!(call("sim.state", json!({}))
            .unwrap_err()
            .contains("sim_attach"));
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
            "no simulator is attached; call sim_attach first"
        );
        let attached = call("sim.attach", json!({})).expect("attach");
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

    /// Drives the fixture app the way an agent would, through every kind of tool:
    /// `pnpm build:sim-fixture`, then
    /// `SIKEMUX_SIM_EXECUTABLE=… SIKEMUX_SIM_FIXTURE=…/SimFixture.app cargo test --lib simulator -- --ignored`.
    #[test]
    #[ignore = "needs the sikemux-sim helper, Xcode's simulators and the fixture app"]
    fn an_agent_drives_the_fixture_app() {
        let _turn = super::real_device();
        let fixture = std::env::var("SIKEMUX_SIM_FIXTURE")
            .expect("SIKEMUX_SIM_FIXTURE names the built SimFixture.app");
        let manager = real_helper();
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
        let device = std::env::var("SIKEMUX_SIM_DEVICE").ok();
        call(
            "sim.attach",
            device.map_or_else(|| json!({}), |device| json!({ "device": device })),
        );
        call("sim.install", json!({ "path": fixture }));

        let opened = call(
            "sim.launch",
            json!({ "bundleId": "com.nodelike.sikemux.simfixture" }),
        );
        assert_eq!(opened["app"], json!("SimFixture"), "{opened}");
        assert!(shows(&opened, "Count: 0"), "{opened}");

        let tapped = call("sim.tap", json!({ "label": "Add one" }));
        assert!(shows(&tapped, "Count: 1"), "{tapped}");

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
        let _turn = super::real_device();
        let manager = real_helper();
        let call = |method: &str, params: serde_json::Value| {
            run_now(&manager, "agent-real", "/tmp", method, &params)
                .unwrap_or_else(|error| panic!("{method}: {error}"))
        };
        let attached = call(
            "sim.attach",
            std::env::var("SIKEMUX_SIM_DEVICE")
                .map_or_else(|_| json!({}), |device| json!({ "device": device })),
        );
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
