//! SnapShots: a picture of the window the person works in, in another app, handed to a chat with
//! the app's name, the window's title and, when macOS lets Sikemux read it, the text the window
//! shows, so the agent knows what it looks at.

// Elsewhere every SnapShot is refused, so what one holds is never made.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod macos;

use gpui_kit::{AppContext, Context, Window};

use crate::chat::Made;
use crate::toast;
use crate::workbench::Workbench;

#[cfg(target_os = "macos")]
pub use hotkey::{listen, register};

/// The command, which its shortcut also runs from any app.
pub const COMMAND: &str = "chat.snapshot";

/// A window's picture and what is known of it.
pub struct Shot {
    pub jpeg: Vec<u8>,
    pub app: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Pixels in the picture per point of the window.
    pub scale: f64,
    pub text: Text,
}

/// What the window shows as text, read through macOS's accessibility.
pub enum Text {
    Read { elements: Vec<Element>, more: usize },
    NotAllowed,
    Unread,
}

pub struct Element {
    pub role: String,
    pub text: String,
    /// Where it sits in points from the window's top left: x, y, width, height.
    pub frame: [f64; 4],
}

/// Why no picture was taken.
pub enum Missed {
    /// Screen Recording is not allowed yet; macOS has been asked.
    NotAllowed,
    NoWindow,
    Unsupported,
    Failed(String),
}

#[cfg(target_os = "macos")]
fn capture() -> Result<Shot, Missed> {
    macos::capture()
}

#[cfg(not(target_os = "macos"))]
fn capture() -> Result<Shot, Missed> {
    Err(Missed::Unsupported)
}

/// The name of the app whose window a SnapShot from Sikemux takes.
pub fn last_app() -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos::last_app()
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty());
    #[cfg(not(target_os = "macos"))]
    None
}

impl Workbench {
    /// Takes the window in front of every other app's and hands it to `agent`'s next message, or
    /// to the project's front chat. `bring_forward` brings Sikemux forward after, as when its
    /// shortcut was pressed in another app.
    pub fn snapshot(
        &mut self,
        agent: Option<String>,
        bring_forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let taken = cx.background_spawn(async { capture() });
        cx.spawn_in(window, async move |bench, cx| {
            let shot = taken.await;
            let _ = bench.update_in(cx, |bench, window, cx| {
                if bring_forward {
                    cx.activate(true);
                }
                let shot = match shot {
                    Ok(shot) => shot,
                    Err(missed) => return bench.notify(missed_note(missed), cx),
                };
                let app = shot.app.clone();
                let made = made(shot);
                let Some(agent) = agent else {
                    let went = bench.hand_to_front_chat(made, "SnapShots", window, cx);
                    if let Some(chat) = went.filter(|_| bring_forward) {
                        let note = t!("SnapShot of {app} added to {chat}", app = app, chat = chat);
                        bench.notify(toast::success(note), cx);
                    }
                    return;
                };
                bench.attach_picture(&agent, made, window, cx);
            });
        })
        .detach();
    }
}

fn missed_note(missed: Missed) -> gpui_kit::component::notification::Notification {
    match missed {
        Missed::NotAllowed => toast::info(
            t!("Allow Sikemux under Screen Recording in System Settings, then reopen it to take SnapShots"),
        ),
        Missed::NoWindow => toast::info(t!("No other app's window is open to take")),
        Missed::Unsupported => toast::info(t!("SnapShots need macOS 14 or later")),
        Missed::Failed(error) => toast::error(t!("The SnapShot failed: {error}", error = error)),
    }
}

fn made(shot: Shot) -> Made {
    let name = format!(
        "snapshot-{}.jpg",
        chrono::Local::now().format("%Y%m%d-%H%M%S%3f")
    );
    let Shot {
        jpeg,
        app,
        title,
        width,
        height,
        scale,
        text,
    } = shot;
    Made {
        name,
        mime: "image/jpeg",
        bytes: jpeg,
        about: Box::new(move |path| {
            let file = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let text = about(&file, &app, &title, (width, height, scale), &text);
            (format!("SnapShot of {app}"), text)
        }),
    }
}

/// What the agent alone is told about the SnapShot kept as `file`.
fn about(file: &str, app: &str, title: &str, size: (u32, u32, f64), text: &Text) -> String {
    let (width, height, scale) = size;
    let named = match title.is_empty() {
        true => String::new(),
        false => format!(" \"{title}\""),
    };
    let mut about = format!(
        "{file} is a picture I took of {app}'s window{named}, {width} × {height} pixels, \
         {scale} pixels to a point."
    );
    match text {
        Text::Read { elements, more } if !elements.is_empty() => {
            about.push_str(
                "\n\nWhat the window shows, from its accessibility data, each at x, y, width × \
                 height in points from its top left:",
            );
            for Element { role, text, frame } in elements {
                let [x, y, w, h] = frame.map(f64::round);
                about.push_str(&format!("\n- {role} \"{text}\" at {x}, {y}, {w} × {h}"));
            }
            if *more > 0 {
                about.push_str(&format!("\n- and {more} more"));
            }
        }
        Text::Read { .. } | Text::Unread => {}
        Text::NotAllowed => about.push_str(
            "\n\nIts text was not read: Sikemux is not allowed Accessibility in System Settings.",
        ),
    }
    about
}
