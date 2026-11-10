//! What the agents may use on this Mac: dictation, the island over the notch,
//! and the iOS Simulator.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::Disableable;
use gpui_kit::{div, AnyElement, App, Context, Div, Entity, ParentElement, SharedString, Window};
use serde_json::Value;
use sikemux_simulator::SimStatus;
use sikemux_view::notch::NotchSettings;
use sikemux_view::notch::{NotchDisplays, NotchFullScreen, NotchOpenWith, NotchPeeks};
use sikemux_view::prefs::PreferenceChange;

use crate::settings::rows::{self, Choice};
use crate::settings::Page;
use crate::voice::Phase;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(
        crate::settings::index::Page::Notch,
        "Notch",
        "island dynamic menu bar notch overlay agents running",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Show the island",
        "notch dynamic island enable disable",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Show on",
        "display monitor external screen",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Open with",
        "hover click",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "In full-screen apps",
        "fullscreen hide",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Peek when",
        "peek finished needs you",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Answer permissions in the notch",
        "allow deny permission",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Sound when an agent needs you",
        "sound alert",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Haptic feedback",
        "haptics trackpad force touch tick vibration swipe",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Notch,
        "Notch",
        "Step aside for Sikemux Dev",
        "dev build development",
    ),
    crate::settings::index::section(
        crate::settings::index::Page::Voice,
        "Voice",
        "dictation microphone speech talk transcribe",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Voice,
        "Voice",
        "Dictate with right Option",
        "dictation microphone speech push to talk hold",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Voice,
        "Voice",
        "Speech model",
        "parakeet download neural engine",
    ),
    crate::settings::index::section(
        crate::settings::index::Page::Simulator,
        "iOS Simulator",
        "iphone ipad xcode devicehub simulator phone mobile bezel frame buttons",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "iOS Simulator",
        "Let agents drive simulators and Android devices",
        "iphone android tap swipe app test mobile",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "iOS Simulator",
        "Xcode",
        "xcode-select developer tools",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "iOS Simulator",
        "iOS runtimes",
        "ios version runtime",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "iOS Simulator",
        "Simulator helper",
        "download idb",
    ),
    crate::settings::index::section(
        crate::settings::index::Page::Simulator,
        "Android Emulator",
        "android emulator avd pixel phone sdk adb",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "Android Emulator",
        "Android SDK",
        "sdk android_home set up install download",
    ),
    crate::settings::index::row(
        crate::settings::index::Page::Simulator,
        "Android Emulator",
        "Virtual devices",
        "avd pixel emulator",
    ),
];

pub struct Fields {
    displays: Choice,
    open_with: Choice,
    full_screen: Choice,
    peeks: Choice,
    /// Whether the simulator can run here, once asked.
    sim_status: Option<SimStatus>,
    /// Xcode, its iOS runtimes and the helper, once read; none when they could not be.
    sim_setup: Option<Option<Value>>,
    /// The Android SDK, its emulator and virtual devices, once read.
    android: Option<Value>,
}

/// Asks whether the simulator can run here and, when it can, what it would run with.
pub fn read_simulator(bench: &Workbench, cx: &mut Context<Workbench>) {
    let android = bench.sims.read(cx).android_info();
    cx.spawn(async move |bench, cx| {
        let android = android.await;
        let _ = bench.update(cx, |bench, cx| {
            super::change_fields(bench, cx, |fields, _| fields.device.android = android);
        });
    })
    .detach();
    let status = bench.sims.read(cx).status();
    cx.spawn(async move |bench, cx| {
        let status = status.await;
        let usable = status.as_ref().is_some_and(SimStatus::usable);
        let setup = match usable {
            true => bench
                .update(cx, |bench, cx| bench.sims.read(cx).setup())
                .ok(),
            false => None,
        };
        let setup = match setup {
            Some(setup) => Some(setup.await),
            None => None,
        };
        let _ = bench.update(cx, |bench, cx| {
            super::change_fields(bench, cx, |fields, _| {
                fields.device.sim_status = status;
                fields.device.sim_setup = setup;
            });
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
fn set_notch(bench: &Workbench, edit: impl FnOnce(&mut NotchSettings)) {
    let mut notch = bench.preferences().notch.clone();
    edit(&mut notch);
    crate::settings::set(bench, PreferenceChange::Notch(notch));
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let notch = bench.preferences().notch.clone();
        let choice = |labels: Vec<&'static str>,
                      chosen: usize,
                      pick: fn(&mut NotchSettings, usize),
                      window: &mut Window,
                      cx: &mut Context<Workbench>| {
            Choice::new(
                labels,
                chosen,
                move |bench, at, _| set_notch(bench, |notch| pick(notch, at)),
                window,
                cx,
            )
        };
        Self {
            sim_status: None,
            sim_setup: None,
            android: None,
            displays: choice(
                vec![
                    "Every display",
                    "Built-in display only",
                    "The display under the pointer",
                ],
                match notch.displays {
                    NotchDisplays::All => 0,
                    NotchDisplays::BuiltIn => 1,
                    NotchDisplays::Pointer => 2,
                },
                |notch, at| {
                    notch.displays = [
                        NotchDisplays::All,
                        NotchDisplays::BuiltIn,
                        NotchDisplays::Pointer,
                    ][at]
                },
                window,
                cx,
            ),
            open_with: choice(
                vec!["Resting the pointer", "Clicking"],
                match notch.open_with {
                    NotchOpenWith::Hover => 0,
                    NotchOpenWith::Click => 1,
                },
                |notch, at| notch.open_with = [NotchOpenWith::Hover, NotchOpenWith::Click][at],
                window,
                cx,
            ),
            full_screen: choice(
                vec![
                    "Hide unless an agent needs you",
                    "Always show",
                    "Always hide",
                ],
                match notch.full_screen {
                    NotchFullScreen::NeedsYou => 0,
                    NotchFullScreen::Always => 1,
                    NotchFullScreen::Never => 2,
                },
                |notch, at| {
                    notch.full_screen = [
                        NotchFullScreen::NeedsYou,
                        NotchFullScreen::Always,
                        NotchFullScreen::Never,
                    ][at]
                },
                window,
                cx,
            ),
            peeks: choice(
                vec![
                    "An agent needs you or finishes",
                    "An agent needs you",
                    "Never",
                ],
                match notch.peeks {
                    NotchPeeks::All => 0,
                    NotchPeeks::NeedsYou => 1,
                    NotchPeeks::Never => 2,
                },
                |notch, at| {
                    notch.peeks = [NotchPeeks::All, NotchPeeks::NeedsYou, NotchPeeks::Never][at]
                },
                window,
                cx,
            ),
        }
    }
}

/// A switch for one of the island's settings, off while the island is.
fn notch_switch(
    id: &'static str,
    set: fn(&mut NotchSettings, bool),
    on: bool,
    enabled: bool,
    this: &Entity<Workbench>,
) -> Switch {
    let this = this.clone();
    Switch::new(id).checked(on).disabled(!enabled).on_change(
        move |on: &bool, _: &mut Window, cx: &mut App| {
            let on = *on;
            this.update(cx, |bench, _| set_notch(bench, |notch| set(notch, on)))
        },
    )
}

/// What the simulator would run with, or why it cannot run here.
fn simulator_rows(fields: &Fields, cx: &App) -> Vec<Div> {
    let unavailable = fields.sim_status.as_ref().and_then(|status| {
        (!status.usable()).then(|| {
            status
                .reason
                .clone()
                .unwrap_or_else(|| "The iOS Simulator is not available here.".to_owned())
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

/// The Android SDK in use and its devices, or the setup that would bring them.
fn android_rows(
    bench: &Workbench,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<Div> {
    let about = |text: String| -> Option<SharedString> { Some(text.into()) };
    if let Some((step, fraction)) = bench.sims.read(cx).android_setup() {
        let percent = (fraction * 100.).round();
        return vec![rows::row_with(
            None,
            "Android SDK",
            about(format!("Setting up… {percent}% · {step}")),
            div(),
            cx,
        )];
    }
    let Some(android) = &fields.android else {
        return vec![rows::row_with(
            None,
            "Android SDK",
            about("Checking…".into()),
            div(),
            cx,
        )];
    };
    let offer = android["offer"].as_u64();
    let set_up = offer.map(|bytes| {
        let this = this.clone();
        Button::new("android-set-up")
            .primary()
            .compact()
            .label(format!("Set up · {:.1} GB", bytes as f64 / 1e9))
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    bench
                        .sims
                        .update(cx, |sims, cx| sims.set_up_android(None, cx))
                })
            })
    });
    let sdk = match android["sdk"].as_str() {
        Some(path) => crate::ui::path::tilde(path, crate::ui::path::this_home().as_deref()),
        None => "Not found. Sikemux can set up its own, with no Android Studio or Java.".into(),
    };
    let mut found = vec![rows::row_with(
        None,
        "Android SDK",
        about(sdk),
        div().children(set_up),
        cx,
    )];
    if let Some(version) = android["emulator"].as_str() {
        found.push(rows::row_with(
            None,
            "Emulator",
            about(version.to_owned()),
            div(),
            cx,
        ));
    }
    let devices: Vec<&str> = android["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !devices.is_empty() {
        found.push(rows::row_with(
            None,
            "Virtual devices",
            about(devices.join(" · ")),
            div(),
            cx,
        ));
    }
    found
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    page: Page,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
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
            rows::switch("voice-dictation", PreferenceChange::VoiceDictation, dictating, this)
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
            notch_switch("notch-enabled", |notch, on| notch.enabled = on, on, true, this),
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
            notch_switch("notch-answer", |notch, on| notch.answer_in_notch = on, notch.answer_in_notch, on, this),
            cx,
        ))
        .child(rows::row(
            "Sound when an agent needs you",
            None,
            notch_switch("notch-sound", |notch, on| notch.sound = on, notch.sound, on, this),
            cx,
        ))
        .child(rows::row(
            "Haptic feedback",
            Some("A tick on a Force Touch trackpad as the pointer reaches the island, a swipe opens or closes it, or an agent needs you."),
            notch_switch("notch-haptics", |notch, on| notch.haptics = on, notch.haptics, on, this),
            cx,
        ))
        .child(rows::row(
            "Step aside for Sikemux Dev",
            Some("While a dev build runs, its island takes the notch."),
            notch_switch("notch-yield", |notch, on| notch.yield_to_dev = on, notch.yield_to_dev, on, this),
            cx,
        ));

    let simulator = div()
        .child(rows::row(
            "Let agents drive simulators and Android devices",
            Some("Agents tap, type and read iOS simulators, Android emulators and phones plugged in with USB debugging without asking, as they do browser tabs, and their screenshots go to the agent's provider. Applies to agents started after a change."),
            rows::switch("ios-simulator", PreferenceChange::IosSimulator, preferences.ios_simulator, this),
            cx,
        ))
        .child(rows::row(
            "Show the device's frame",
            Some("Draws the phone's bezel and side buttons around its screen, as Simulator does. Without it the screen gets a little more room."),
            rows::switch("simulator-frame", PreferenceChange::SimulatorFrame, preferences.simulator_frame, this),
            cx,
        ))
        .children(simulator_rows(fields, cx));

    let section = match page {
        Page::Voice => rows::section("Voice", None, None, lit("Voice"), voice, cx),
        Page::Notch => rows::section("Notch", None, None, lit("Notch"), island, cx),
        _ => {
            let android = rows::section(
                "Android Emulator",
                None,
                None,
                lit("Android Emulator"),
                div().children(android_rows(bench, fields, this, cx)),
                cx,
            );
            let ios = rows::section(
                "iOS Simulator",
                None,
                None,
                lit("iOS Simulator"),
                simulator,
                cx,
            );
            return vec![ios, android];
        }
    };
    vec![section]
}
