//! Drives the window from step files, for the real-window checks of the test bundle only: it
//! is built with the `drive` feature, which no try-out or release build turns on. Steps come
//! over the socket named by `SIKEMUX_GPUI_DRIVE` and go through the window's own event
//! dispatch, so nothing is posted to the system and no Accessibility grant is needed.
//!
//! Steps, one per line and ended by a line `end`, in points from the window's top-left corner:
//!   activate | move x y | click x y | double x y | right x y | drag x1 y1 x2 y2 [steps]
//!   key cmd+shift+p | text some words | scroll x y dx dy [steps] [momentum] | wait seconds
//!   state, answered with what the desks hold and where the keys are, as JSON on its line
//!   register, which takes the core's window calls without activating the window
//!   resize width height, in points

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

use gpui_kit::{
    point, px, AnyWindowHandle, App, AsyncApp, Entity, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollWheelEvent, TouchPhase,
};
use tokio::sync::mpsc;

use crate::workbench::Workbench;

const SETTLE: Duration = Duration::from_millis(60);
const FRAME: Duration = Duration::from_millis(16);
/// As long as a person holds a click: GPUI listens for the release only once a frame has
/// drawn after the press.
const PRESS: Duration = Duration::from_millis(80);

/// Whether the test app opens without taking the person's keys, for checks run while they work.
pub fn stays_behind() -> bool {
    std::env::var_os("SIKEMUX_GPUI_DRIVE_BEHIND").is_some()
}

/// Listens for step files while the test bundle runs, and answers each with what it did.
pub fn listen(window: AnyWindowHandle, bench: Entity<Workbench>, cx: &mut App) {
    let Some(path) = std::env::var_os("SIKEMUX_GPUI_DRIVE") else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => return eprintln!("sikemux: the window driver could not listen: {error}"),
    };
    let (runs, mut asked) = mpsc::unbounded_channel::<(String, UnixStream)>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let Ok(reading) = stream.try_clone() else {
                continue;
            };
            let steps: Vec<String> = BufReader::new(reading)
                .lines()
                .map_while(Result::ok)
                .take_while(|line| line.trim() != "end")
                .collect();
            if runs.send((steps.join("\n"), stream)).is_err() {
                break;
            }
        }
    });
    cx.spawn(async move |cx| {
        while let Some((steps, mut stream)) = asked.recv().await {
            for line in steps.lines().map(str::trim) {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let said = match run(line, window, &bench, cx).await {
                    Ok(None) => format!("did: {line}\n"),
                    Ok(Some(answer)) => format!("did: {line} {answer}\n"),
                    Err(problem) => format!("{problem}: {line}\n"),
                };
                let _ = stream.write_all(said.as_bytes());
                cx.background_executor().timer(SETTLE).await;
            }
            let _ = stream.write_all(b"done\n");
        }
    })
    .detach();
}

async fn run(
    line: &str,
    window: AnyWindowHandle,
    bench: &Entity<Workbench>,
    cx: &mut AsyncApp,
) -> Result<Option<String>, &'static str> {
    if line == "state" {
        return window
            .update(cx, |_, window, cx| bench.read(cx).drive_state(window, cx))
            .map(|state| Some(state.to_string()))
            .map_err(|_| "the window is gone");
    }
    if line == "register" {
        bench.read_with(cx, |bench, _| {
            let _ = bench.asks.send(crate::core::Ask::RegisterWindow);
        });
        return Ok(None);
    }
    acted(line, window, cx).await.map(|()| None)
}

async fn acted(line: &str, window: AnyWindowHandle, cx: &mut AsyncApp) -> Result<(), &'static str> {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    let words: Vec<&str> = rest.split_whitespace().collect();
    let number = |at: usize| words.get(at).and_then(|word| word.parse::<f32>().ok());
    let at = |from: usize| Some(point(px(number(from)?), px(number(from + 1)?)));
    match verb {
        "activate" => send(window, cx, |window, _| window.activate_window()),
        "resize" => {
            let to = at(0).ok_or("bad step")?;
            send(window, cx, |window, _| {
                window.resize(gpui_kit::size(to.x, to.y));
            })
        }
        "wait" => {
            let seconds = number(0).unwrap_or(1.);
            cx.background_executor()
                .timer(Duration::from_secs_f32(seconds))
                .await;
            Ok(())
        }
        "move" => {
            let position = at(0).ok_or("bad step")?;
            input(window, cx, moved(position, None))
        }
        "click" | "double" | "right" => {
            let position = at(0).ok_or("bad step")?;
            let button = if verb == "right" {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            input(window, cx, moved(position, None))?;
            let clicks = if verb == "double" { 2 } else { 1 };
            for count in 1..=clicks {
                input(window, cx, down(position, button, count))?;
                cx.background_executor().timer(PRESS).await;
                input(window, cx, up(position, button, count))?;
            }
            Ok(())
        }
        "drag" => {
            let (from, to) = (at(0).ok_or("bad step")?, at(2).ok_or("bad step")?);
            let steps = number(4).map_or(20, |steps| steps.max(1.) as usize);
            input(window, cx, moved(from, None))?;
            input(window, cx, down(from, MouseButton::Left, 1))?;
            for step in 1..=steps {
                let along = step as f32 / steps as f32;
                let position = from + (to - from) * along;
                input(window, cx, moved(position, Some(MouseButton::Left)))?;
                cx.background_executor().timer(FRAME).await;
            }
            input(window, cx, up(to, MouseButton::Left, 1))
        }
        "key" => {
            let keystroke = keystroke(rest).ok_or("bad step")?;
            send(window, cx, |window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
        }
        "text" => {
            for character in rest.chars() {
                let typed = Keystroke {
                    modifiers: Modifiers::default(),
                    key: character.to_lowercase().to_string(),
                    key_char: Some(character.to_string()),
                };
                send(window, cx, |window, cx| {
                    window.dispatch_keystroke(typed, cx);
                })?;
                cx.background_executor().timer(FRAME).await;
            }
            Ok(())
        }
        "scroll" => {
            let position = at(0).ok_or("bad step")?;
            let delta = at(2).ok_or("bad step")?;
            let steps = number(4).map_or(10, |steps| steps.max(1.) as usize);
            let piece = delta / steps as f32;
            input(window, cx, moved(position, None))?;
            input(
                window,
                cx,
                wheel(position, Point::default(), TouchPhase::Started),
            )?;
            for _ in 0..steps {
                input(window, cx, wheel(position, piece, TouchPhase::Moved))?;
                cx.background_executor().timer(FRAME).await;
            }
            input(
                window,
                cx,
                wheel(position, Point::default(), TouchPhase::Ended),
            )?;
            if words.contains(&"momentum") {
                for fade in [0.9, 0.75, 0.6, 0.45, 0.3, 0.15] {
                    input(window, cx, wheel(position, piece * fade, TouchPhase::Moved))?;
                    cx.background_executor().timer(FRAME).await;
                }
            }
            Ok(())
        }
        _ => Err("unknown step"),
    }
}

/// `cmd+shift+p` as the keystroke GPUI parses, with the step files' key names.
fn keystroke(combo: &str) -> Option<Keystroke> {
    let parts: Vec<&str> = combo.trim().split('+').collect();
    let (key, held) = parts.split_last()?;
    let key = match *key {
        "return" => "enter",
        "esc" => "escape",
        "delete" => "backspace",
        "forwarddelete" => "delete",
        key => key,
    };
    let held: Vec<&str> = held.iter().map(|part| part.trim()).collect();
    let spelled = held
        .iter()
        .copied()
        .chain([key])
        .collect::<Vec<_>>()
        .join("-");
    Keystroke::parse(&spelled).ok()
}

fn moved(position: Point<Pixels>, pressed_button: Option<MouseButton>) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position,
        pressed_button,
        modifiers: Modifiers::default(),
    })
}

fn down(position: Point<Pixels>, button: MouseButton, click_count: usize) -> PlatformInput {
    PlatformInput::MouseDown(MouseDownEvent {
        button,
        position,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    })
}

fn up(position: Point<Pixels>, button: MouseButton, click_count: usize) -> PlatformInput {
    PlatformInput::MouseUp(MouseUpEvent {
        button,
        position,
        modifiers: Modifiers::default(),
        click_count,
    })
}

fn wheel(position: Point<Pixels>, delta: Point<Pixels>, touch_phase: TouchPhase) -> PlatformInput {
    PlatformInput::ScrollWheel(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(delta),
        modifiers: Modifiers::default(),
        touch_phase,
    })
}

fn input(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    event: PlatformInput,
) -> Result<(), &'static str> {
    send(window, cx, |window, cx| {
        window.dispatch_event(event, cx);
    })
}

fn send(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    act: impl FnOnce(&mut gpui_kit::Window, &mut App),
) -> Result<(), &'static str> {
    window
        .update(cx, |_, window, cx| act(window, cx))
        .map_err(|_| "the window is gone")
}

thread_local! {
    static SPOTS: RefCell<BTreeMap<String, [f32; 4]>> = const { RefCell::new(BTreeMap::new()) };
}

/// An invisible mark over its parent that notes where the parent was last drawn, so a check
/// can click a control it cannot otherwise find.
pub fn spot(name: String) -> gpui_kit::AnyElement {
    use gpui_kit::{IntoElement, Styled};
    gpui_kit::canvas(
        move |bounds, window, _| {
            let shown = bounds.intersect(&window.content_mask().bounds);
            SPOTS.with(|spots| {
                let mut spots = spots.borrow_mut();
                if shown.size.width <= gpui_kit::px(0.) || shown.size.height <= gpui_kit::px(0.) {
                    spots.remove(&name);
                    return;
                }
                let frame = [
                    shown.left(),
                    shown.top(),
                    shown.size.width,
                    shown.size.height,
                ];
                spots.insert(name.clone(), frame.map(f32::from));
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Forgets the spots whose names start with `prefix`, for a view about to draw them again, so
/// one it no longer draws is not left where it last was.
pub fn forget_spots(prefix: &str) {
    SPOTS.with(|spots| {
        spots
            .borrow_mut()
            .retain(|name, _| !name.starts_with(prefix))
    });
}

/// Every spot's last frame, left, top, width and height, by name: the part of it not clipped
/// away, as where a click can land; one scrolled wholly out of view has none.
pub fn spots() -> serde_json::Value {
    SPOTS.with(|spots| serde_json::json!(*spots.borrow()))
}

/// Writes when something happened, in milliseconds since the epoch, to the test bundle's log,
/// so a real-window run can tell when a core answer reached each thread.
pub fn trace(what: std::fmt::Arguments) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    eprintln!("sikemux trace {now} {what}");
}
