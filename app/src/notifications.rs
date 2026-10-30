//! Tells the person, while Sikemux is in the background, when an agent needs them or
//! finished: a system notification that brings the agent forward when clicked, and the
//! Dock or launcher icon's bounce and badge.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::Context;
use sikemux_wire::protocol::{AgentGroup, DeviceView, PaneKind, SessionKind};
use tokio::sync::mpsc;

use crate::toast;
use crate::workbench::Workbench;

/// As long as the Mac shows it.
const KEPT_RUNNING_SHOWN: Duration = Duration::from_secs(12);

/// The keys `keymap` binds to Quit and Stop Everything.
const STOP_EVERYTHING_KEYS: &str = if cfg!(target_os = "macos") {
    "⌥⌘Q"
} else {
    "Ctrl+Alt+Q"
};

/// What the test row and the first launch post, to show what a notice looks like.
const TEST: (&str, &str) = (
    "Sikemux notifications are on",
    "This is how an agent will tell you it needs you or has finished.",
);

/// Posts a notification; clicking it brings Sikemux forward on the agent it is about.
pub fn post(title: String, body: String, agent: Option<String>, cx: &mut Context<Workbench>) {
    follow_clicks(cx);
    #[cfg(target_os = "macos")]
    macos::post(&title, &body, agent.as_deref());
    #[cfg(not(target_os = "macos"))]
    cx.show_system_notification(gpui_kit::SystemNotification {
        tag: agent.unwrap_or_default().into(),
        title: title.into(),
        body: body.into(),
        actions: Vec::new(),
    });
}

/// Where a click on a notification goes, with the agent it is about.
static CLICKS: OnceLock<mpsc::UnboundedSender<Option<String>>> = OnceLock::new();

/// Brings Sikemux forward on each clicked notification's agent, heard once for every one.
fn follow_clicks(cx: &mut Context<Workbench>) {
    if CLICKS.get().is_some() {
        return;
    }
    let (clicks, mut clicked) = mpsc::unbounded_channel();
    if CLICKS.set(clicks).is_err() {
        return;
    }
    #[cfg(not(target_os = "macos"))]
    cx.on_system_notification_response(|clicked, _| {
        if let Some(clicks) = CLICKS.get() {
            let _ = clicks.send(Some(clicked.tag.to_string()).filter(|agent| !agent.is_empty()));
        }
    });
    cx.spawn(async move |bench, cx| {
        while let Some(agent) = clicked.recv().await {
            let shown = bench.update(cx, |bench, cx| {
                cx.activate(true);
                if let Some(agent) = agent {
                    bench.open_agent(&agent);
                }
            });
            if shown.is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod macos {
    use std::cell::OnceCell;
    use std::sync::Once;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, ClassType, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSSound;
    use objc2_foundation::{
        ns_string, NSData, NSDictionary, NSObject, NSString, NSUserNotification,
        NSUserNotificationCenter, NSUserNotificationCenterDelegate,
    };

    define_class!(
        /// Hears every click on the app's notifications.
        // SAFETY: a plain NSObject subclass that adds no dealloc and is only used on the
        // main thread.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        struct ClickHandler;

        // SAFETY: every NSObject subclass already conforms to NSObjectProtocol.
        unsafe impl NSObjectProtocol for ClickHandler {}

        // SAFETY: both methods have the delegate protocol's exact signatures.
        unsafe impl NSUserNotificationCenterDelegate for ClickHandler {
            #[unsafe(method(userNotificationCenter:didActivateNotification:))]
            fn clicked(
                &self,
                center: &NSUserNotificationCenter,
                notification: &NSUserNotification,
            ) {
                center.removeDeliveredNotification(notification);
                let agent = notification
                    .userInfo()
                    .and_then(|info| info.objectForKey(agent_key()))
                    .and_then(|agent| agent.downcast::<NSString>().ok())
                    .map(|agent| agent.to_string());
                if let Some(clicks) = super::CLICKS.get() {
                    let _ = clicks.send(agent);
                }
            }

            #[unsafe(method(userNotificationCenter:shouldPresentNotification:))]
            fn shown_in_front(
                &self,
                _center: &NSUserNotificationCenter,
                _notification: &NSUserNotification,
            ) -> bool {
                true
            }
        }
    );

    fn agent_key() -> &'static NSString {
        ns_string!("agent")
    }

    /// macOS gives an app with no bundle, such as a run from the build folder, no notification
    /// center.
    fn center() -> Option<Retained<NSUserNotificationCenter>> {
        // SAFETY: the class method takes nothing and returns the shared center or nil.
        unsafe {
            msg_send![
                NSUserNotificationCenter::class(),
                defaultUserNotificationCenter
            ]
        }
    }

    pub(super) fn post(title: &str, body: &str, agent: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(center) = center() else {
            return;
        };
        static LISTENING: Once = Once::new();
        LISTENING.call_once(|| {
            // SAFETY: NSObject's `init` takes nothing and returns the object it was sent to.
            let handler: Retained<ClickHandler> =
                unsafe { msg_send![mtm.alloc::<ClickHandler>(), init] };
            // SAFETY: the handler is never released, so it outlives the center's unretained
            // reference to it.
            unsafe {
                center.setDelegate(Some(ProtocolObject::from_ref(&*Retained::into_raw(
                    handler,
                ))));
            }
        });
        let notification = NSUserNotification::new();
        notification.setTitle(Some(&NSString::from_str(title)));
        notification.setInformativeText(Some(&NSString::from_str(body)));
        if let Some(agent) = agent {
            let agent = NSString::from_str(agent);
            let info = NSDictionary::<NSString, AnyObject>::from_slices(&[agent_key()], &[&agent]);
            // SAFETY: the dictionary holds only strings, which the notification can store.
            unsafe { notification.setUserInfo(Some(&info)) };
        }
        center.deliverNotification(&notification);
        chime(mtm);
    }

    /// A short, soft tone, so a notice is heard whatever macOS's sound settings say.
    fn chime(mtm: MainThreadMarker) {
        thread_local! {
            static TONE: OnceCell<Option<Retained<NSSound>>> = const { OnceCell::new() };
        }
        TONE.with(|tone| {
            let tone = tone.get_or_init(|| {
                NSSound::initWithData(mtm.alloc::<NSSound>(), &NSData::from_vec(super::tone()))
            });
            if let Some(tone) = tone {
                tone.stop();
                tone.play();
            }
        });
    }
}

/// The old app's notice tone as a WAV: 880 Hz for 0.16 s, rising in 10 ms and fading by 150 ms.
#[cfg(target_os = "macos")]
fn tone() -> Vec<u8> {
    const RATE: u32 = 44_100;
    let samples = RATE * 16 / 100;
    let level = |at: f32| match at {
        at if at < 0.01 => 0.0001 * (0.2f32 / 0.0001).powf(at / 0.01),
        at if at < 0.15 => 0.2 * (0.0001f32 / 0.2).powf((at - 0.01) / 0.14),
        _ => 0.0,
    };
    let bytes = samples * 2;
    let mut wav = Vec::with_capacity(44 + bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in 0..samples {
        let at = sample as f32 / RATE as f32;
        let value = (at * 880.0 * std::f32::consts::TAU).sin() * level(at);
        wav.extend_from_slice(&((value * f32::from(i16::MAX)) as i16).to_le_bytes());
    }
    wav
}

/// Posts what the core said about an agent, while the person is away from Sikemux and
/// asked to hear.
pub fn notice(
    bench: &Workbench,
    title: String,
    body: String,
    agent: String,
    cx: &mut Context<Workbench>,
) {
    if !bench.preferences().agent_notifications || cx.active_window().is_some() {
        return;
    }
    if let Some(window) = cx.windows().first().copied() {
        let _ = window.update(cx, |_, window, _| window.request_attention());
    }
    post(title, body, Some(agent), cx);
}

pub fn test(cx: &mut Context<Workbench>) {
    post(TEST.0.into(), TEST.1.into(), None, cx);
}

/// macOS asks whether an app may notify only the first time it posts, so the first launch
/// with notifications on posts a greeting to put that question up front.
pub fn introduce(bench: &Workbench, cx: &mut Context<Workbench>) {
    let preferences = bench.preferences();
    if !preferences.agent_notifications || preferences.notifications_introduced {
        return;
    }
    crate::settings::set(bench, "notificationsIntroduced", true);
    test(cx);
}

/// Says once, the first time Sikemux opens onto terminals, tasks or chats that ran on while it
/// was closed, how to stop them.
pub fn kept_running(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if bench.kept_running_told {
        return;
    }
    let (Some(view), Some(preferences)) = (&bench.view, bench.preferences_read()) else {
        return;
    };
    let unshown = !preferences.kept_running_notice_shown;
    let mut shown = HashSet::new();
    let layout = &view.layout;
    let strips = layout
        .projects
        .values()
        .chain(layout.hosts.values())
        .chain(std::iter::once(&layout.terminals));
    for strip in strips {
        for tab in &strip.tabs {
            shown.extend(
                crate::panes::views(&tab.pane)
                    .filter(|view| view.kind == PaneKind::Terminal)
                    .map(|view| view.target.as_str()),
            );
        }
    }
    let running = |kind: SessionKind| {
        view.sessions
            .iter()
            .filter(move |session| session.kind == kind && session.running)
    };
    let terminals = running(SessionKind::Terminal)
        .filter(|session| shown.contains(session.id.to_string().as_str()))
        .count();
    let kept = terminals + running(SessionKind::Task).count() + view.chats.len();
    bench.kept_running_told = true;
    if !unshown || kept == 0 {
        return;
    }
    crate::settings::set(bench, "keptRunningNoticeShown", true);
    bench.notify_for::<KeptRunning>(
        toast::info(format!(
            "Your terminals and agents kept running while Sikemux was closed. Use Quit and Stop Everything ({STOP_EVERYTHING_KEYS}) to stop them."
        )),
        KEPT_RUNNING_SHOWN,
        cx,
    );
}

struct KeptRunning;

/// How many agents wait on the person: those asking for something, and those that finished
/// a turn the person has not seen.
fn waiting(view: &DeviceView) -> usize {
    let waits =
        |group: Option<AgentGroup>| matches!(group, Some(AgentGroup::Blocked | AgentGroup::Done));
    view.workspace
        .agents
        .iter()
        .filter(|agent| {
            let chat = view
                .chats
                .iter()
                .find(|chat| chat.agent_id == agent.id)
                .map(|chat| chat.group);
            let terminal = view
                .sessions
                .iter()
                .find(|session| session.agent_id.as_deref() == Some(agent.id.as_str()))
                .and_then(|session| session.group);
            waits(chat.or(terminal))
        })
        .count()
}

/// Keeps the Dock or launcher icon's badge at the number of agents waiting on the person.
pub fn badge(bench: &mut Workbench) {
    let count = bench.view.as_ref().map_or(0, waiting);
    if count != std::mem::replace(&mut bench.badge, count) {
        show_badge(count);
    }
}

fn show_badge(count: usize) {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        use objc2_foundation::NSString;
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
        NSApplication::sharedApplication(main)
            .dockTile()
            .setBadgeLabel(label.as_deref());
    }
    #[cfg(target_os = "linux")]
    linux::show_badge(count);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = count;
}

/// The count docks such as GNOME's and KDE's draw on a launcher: Unity's launcher entry
/// signal, sent for the desktop file named after the app's id.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::sync::OnceLock;

    use zbus::zvariant::Value;

    static COUNTS: OnceLock<Option<mpsc::Sender<usize>>> = OnceLock::new();

    /// Sent from a thread of its own, which keeps the bus connection: a dock drops the count
    /// once its sender leaves the bus.
    pub(super) fn show_badge(count: usize) {
        let counts = COUNTS.get_or_init(|| {
            let (counts, shown) = mpsc::channel::<usize>();
            std::thread::Builder::new()
                .name("launcher-badge".into())
                .spawn(move || {
                    let Ok(bus) = zbus::blocking::Connection::session() else {
                        return;
                    };
                    let app = format!("application://{}.desktop", crate::core::IDENTIFIER);
                    for count in shown {
                        let properties = HashMap::from([
                            ("count", Value::from(count as i64)),
                            ("count-visible", Value::from(count > 0)),
                        ]);
                        let _ = bus.emit_signal(
                            None::<zbus::names::BusName>,
                            "/com/nodelike/sikemux/launcher",
                            "com.canonical.Unity.LauncherEntry",
                            "Update",
                            &(app.as_str(), properties),
                        );
                    }
                })
                .ok()
                .map(|_| counts)
        });
        if let Some(counts) = counts {
            let _ = counts.send(count);
        }
    }
}
