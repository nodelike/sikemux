//! What the agents may use on this Mac: dictation, the island over the notch,
//! and the iOS Simulator.

use gpui_kit::component::switch::Switch;
use gpui_kit::component::Disableable;
use gpui_kit::{div, AnyElement, App, Context, Div, Entity, ParentElement, SharedString, Window};
use serde_json::Value;
use sikemux_wire::notch::{NotchDisplays, NotchFullScreen, NotchOpenWith, NotchPeeks};

use crate::settings::rows::{self, Choice};
use crate::voice::Phase;
use crate::workbench::Workbench;

pub struct Fields {
    displays: Choice,
    open_with: Choice,
    full_screen: Choice,
    peeks: Choice,
    /// Whether the simulator can run here, once asked.
    sim_status: Option<Value>,
    /// Xcode, its iOS runtimes and the helper, once read; none when they could not be.
    sim_setup: Option<Option<Value>>,
}

/// Asks whether the simulator can run here and, when it can, what it would run with.
pub fn read_simulator(bench: &Workbench, cx: &mut Context<Workbench>) {
    let status = bench.sims.status();
    cx.spawn(async move |bench, cx| {
        let status = status
            .await
            .and_then(|status| serde_json::to_value(status).ok());
        let usable = status
            .as_ref()
            .is_some_and(|status| status["supported"] == true && status["reason"].is_null());
        let setup = match usable {
            true => bench.update(cx, |bench, _| bench.sims.setup()).ok(),
            false => None,
        };
        let setup = match setup {
            Some(setup) => Some(setup.await),
            None => None,
        };
        let _ = bench.update(cx, |bench, cx| {
            if let Some(fields) = super::fields_of(bench) {
                fields.device.sim_status = status;
                fields.device.sim_setup = setup;
            }
            bench.parts.redraw(&bench.parts.stage, cx);
        });
    })
    .detach();
}

fn xcode(path: Option<&str>) -> String {
    match path {
        None => "No Xcode is selected. Install Xcode, or choose one with xcode-select.".into(),
        Some(path) if path.contains("CommandLineTools") => "Only the Command Line Tools are selected, and simulators need Xcode. Install Xcode, or choose it with xcode-select.".into(),
        Some(path) => path.to_owned(),
    }
}

/// Changes one of the island's settings, keeping the rest.
fn set_notch(bench: &Workbench, key: &str, value: impl Into<Value>) {
    let mut notch = serde_json::to_value(bench.preferences().notch).unwrap_or_default();
    notch[key] = value.into();
    crate::settings::set(bench, "notch", notch);
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let notch = bench.preferences().notch;
        let choice = |labels: Vec<&'static str>,
                      words: &'static [&'static str],
                      chosen: usize,
                      key: &'static str,
                      window: &mut Window,
                      cx: &mut Context<Workbench>| {
            Choice::new(
                labels,
                chosen,
                move |bench, at, _| set_notch(bench, key, words[at]),
                window,
                cx,
            )
        };
        Self {
            sim_status: None,
            sim_setup: None,
            displays: choice(
                vec![
                    "Every display",
                    "Built-in display only",
                    "The display under the pointer",
                ],
                &["all", "builtIn", "pointer"],
                match notch.displays {
                    NotchDisplays::All => 0,
                    NotchDisplays::BuiltIn => 1,
                    NotchDisplays::Pointer => 2,
                },
                "displays",
                window,
                cx,
            ),
            open_with: choice(
                vec!["Resting the pointer", "Clicking"],
                &["hover", "click"],
                match notch.open_with {
                    NotchOpenWith::Hover => 0,
                    NotchOpenWith::Click => 1,
                },
                "openWith",
                window,
                cx,
            ),
            full_screen: choice(
                vec![
                    "Hide unless an agent needs you",
                    "Always show",
                    "Always hide",
                ],
                &["needsYou", "always", "never"],
                match notch.full_screen {
                    NotchFullScreen::NeedsYou => 0,
                    NotchFullScreen::Always => 1,
                    NotchFullScreen::Never => 2,
                },
                "fullScreen",
                window,
                cx,
            ),
            peeks: choice(
                vec![
                    "An agent needs you or finishes",
                    "An agent needs you",
                    "Never",
                ],
                &["all", "needsYou", "never"],
                match notch.peeks {
                    NotchPeeks::All => 0,
                    NotchPeeks::NeedsYou => 1,
                    NotchPeeks::Never => 2,
                },
                "peeks",
                window,
                cx,
            ),
        }
    }
}

/// A switch for one of the island's settings, off while the island is.
fn notch_switch(
    id: &'static str,
    key: &'static str,
    on: bool,
    enabled: bool,
    this: &Entity<Workbench>,
) -> Switch {
    let this = this.clone();
    Switch::new(id).checked(on).disabled(!enabled).on_change(
        move |on: &bool, _: &mut Window, cx: &mut App| {
            let on = *on;
            this.update(cx, |bench, _| set_notch(bench, key, on))
        },
    )
}

/// What the simulator would run with, or why it cannot run here.
fn simulator_rows(fields: &Fields, cx: &App) -> Vec<Div> {
    let unavailable = fields.sim_status.as_ref().and_then(|status| {
        let usable = status["supported"] == true && status["reason"].is_null();
        (!usable).then(|| {
            status["reason"]
                .as_str()
                .unwrap_or("The iOS Simulator is not available here.")
                .to_owned()
        })
    });
    let read = |describe: &dyn Fn(&Value) -> String| match &fields.sim_setup {
        None => "Checking…".to_owned(),
        Some(None) => "Could not read the developer tools.".to_owned(),
        Some(Some(setup)) => describe(setup),
    };
    let about = |text: String| -> Option<SharedString> { Some(text.into()) };
    let mut found = vec![rows::row_with(
        None,
        "Xcode",
        about(
            unavailable
                .clone()
                .unwrap_or_else(|| read(&|setup| xcode(setup["xcode"].as_str()))),
        ),
        div(),
        cx,
    )];
    if unavailable.is_none() {
        found.push(rows::row_with(
            None,
            "iOS runtimes",
            about(read(&|setup| {
                let runtimes: Vec<&str> = setup["runtimes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                match runtimes.is_empty() {
                    true => "None installed. Add one in Xcode's Components settings.".into(),
                    false => runtimes.join(", "),
                }
            })),
            div(),
            cx,
        ));
        found.push(rows::row_with(
            None,
            "Simulator helper",
            about(read(&|setup| {
                format!(
                    "The helper that drives simulators is {}.",
                    setup["helper"].as_str().unwrap_or_default()
                )
            })),
            div(),
            cx,
        ));
    }
    found
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let preferences = bench.preferences();
    let dictating = preferences.voice_dictation;
    let (phase, downloading, reason) = {
        let voice = bench.voice.read(cx);
        (voice.phase(), voice.downloading(), voice.reason().cloned())
    };
    let model: SharedString = match (dictating, phase, downloading, reason) {
        (_, Phase::Unsupported, _, reason) => {
            reason.unwrap_or_else(|| "Not available on this host.".into())
        }
        (_, _, _, Some(reason)) => reason,
        (false, _, _, _) => "Parakeet by NVIDIA, run on the Neural Engine. About 600 MB, downloaded when you turn dictation on.".into(),
        (true, Phase::Preparing, Some(fraction), _) => {
            format!("Downloading… {}%", (fraction * 100.).round()).into()
        }
        (true, Phase::Preparing, None, _) => {
            "Preparing for the Neural Engine. The first time takes about half a minute.".into()
        }
        (true, Phase::Off, _, _) => "Not loaded.".into(),
        (true, _, _, _) => "Ready. Speech never leaves this host.".into(),
    };
    let voice = div()
        .child(rows::row(
            "Dictate with right Option",
            Some("Hold right ⌥ and speak. Letting go types what you said into the focused agent or terminal."),
            rows::switch("voice-dictation", "voiceDictation", dictating, this)
                .disabled(phase == Phase::Unsupported),
            cx,
        ))
        .child(rows::row_with(None, "Speech model", Some(model), div(), cx));

    let notch = &preferences.notch;
    let on = notch.enabled;
    let dimmed = |choice: &Choice| choice.view().disabled(!on);
    let island = div()
        .child(rows::row(
            "Show the island",
            Some("It keeps running after Sikemux quits, as your agents do."),
            notch_switch("notch-enabled", "enabled", on, true, this),
            cx,
        ))
        .child(rows::row(
            "Show on",
            Some("A display without a notch gets a pill at its menu bar's height."),
            dimmed(&fields.displays),
            cx,
        ))
        .child(rows::row(
            "Open with",
            Some("Resting waits a moment, so passing the pointer across the menu bar does not open it."),
            dimmed(&fields.open_with),
            cx,
        ))
        .child(rows::row("In full-screen apps", None, dimmed(&fields.full_screen), cx))
        .child(rows::row(
            "Peek when",
            Some("The island grows for a moment without opening."),
            dimmed(&fields.peeks),
            cx,
        ))
        .child(rows::row(
            "Answer permissions in the notch",
            Some("Otherwise a request offers Open in Sikemux instead of Deny and Allow."),
            notch_switch("notch-answer", "answerInNotch", notch.answer_in_notch, on, this),
            cx,
        ))
        .child(rows::row(
            "Sound when an agent needs you",
            None,
            notch_switch("notch-sound", "sound", notch.sound, on, this),
            cx,
        ))
        .child(rows::row(
            "Haptic feedback",
            Some("A tick on a Force Touch trackpad as the pointer reaches the island, a swipe opens or closes it, or an agent needs you."),
            notch_switch("notch-haptics", "haptics", notch.haptics, on, this),
            cx,
        ))
        .child(rows::row(
            "Step aside for Sikemux Dev",
            Some("While a dev build runs, its island takes the notch."),
            notch_switch("notch-yield", "yieldToDev", notch.yield_to_dev, on, this),
            cx,
        ));

    let simulator = div()
        .child(rows::row(
            "Let agents drive the iOS Simulator",
            Some("Agents tap, type and read simulators without asking, as they do browser tabs, and their screenshots go to the agent's provider. Applies to agents started after a change."),
            rows::switch("ios-simulator", "iosSimulator", preferences.ios_simulator, this),
            cx,
        ))
        .child(rows::row(
            "Show the device's frame",
            Some("Draws the phone's bezel and side buttons around its screen, as Simulator does. Without it the screen gets a little more room."),
            rows::switch("simulator-frame", "simulatorFrame", preferences.simulator_frame, this),
            cx,
        ))
        .children(simulator_rows(fields, cx));

    vec![
        rows::section("Voice", None, None, lit("Voice"), voice, cx),
        rows::section(
            "Notch",
            None,
            Some("The island over the MacBook notch shows the agents running, what they need from you, and starts new ones.".into()),
            lit("Notch"),
            island,
            cx,
        ),
        rows::section("iOS Simulator", None, None, lit("iOS Simulator"), simulator, cx),
    ]
}
