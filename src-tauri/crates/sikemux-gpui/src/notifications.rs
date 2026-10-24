//! Tells the person, while Sikemux is in the background, when an agent needs them or
//! finished: a macOS notification that brings the agent forward when clicked, and the
//! Dock icon's bounce and badge.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::Context;
use sikemux_wire::protocol::{AgentGroup, DeviceView, Pane, PaneKind, SessionKind};
use tokio::sync::mpsc;

use crate::toast;
use crate::workbench::Workbench;

/// As long as the Mac shows it.
const KEPT_RUNNING_SHOWN: Duration = Duration::from_secs(12);

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
    let _ = (title, body, agent);
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
    use std::sync::Once;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, ClassType, MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{
        ns_string, NSDictionary, NSObject, NSString, NSUserNotification, NSUserNotificationCenter,
        NSUserNotificationCenterDelegate,
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
    }
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
    let (Some(view), Some(preferences)) = (&bench.view, bench.read("preferences")) else {
        return;
    };
    let unshown = preferences["keptRunningNoticeShown"] == false;
    let mut shown = HashSet::new();
    let layout = &view.layout;
    let strips = layout
        .projects
        .values()
        .chain(layout.hosts.values())
        .chain(std::iter::once(&layout.terminals));
    for strip in strips {
        for tab in &strip.tabs {
            terminals_in(&tab.pane, &mut shown);
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
        toast::info(
            "Your terminals and agents kept running while Sikemux was closed. Use Quit and Stop Everything (⌥⌘Q) to stop them.",
        ),
        KEPT_RUNNING_SHOWN,
        cx,
    );
}

struct KeptRunning;

fn terminals_in<'a>(pane: &'a Pane, found: &mut HashSet<&'a str>) {
    match pane {
        Pane::View {
            kind: PaneKind::Terminal,
            target,
            ..
        } => {
            found.insert(target);
        }
        Pane::View { .. } => {}
        Pane::Split { panes, .. } => panes.iter().for_each(|pane| terminals_in(pane, found)),
    }
}

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

/// Keeps the Dock icon's badge at the number of agents waiting on the person.
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
}
