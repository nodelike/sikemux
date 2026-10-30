//! Draws the window offscreen, as the Metal renderer would draw it, and saves it as a PNG at
//! 2x, so the UI can be checked while no one is at the screen:
//!   cargo run --example shot -- out.png [seconds to wait for the core]
//!     [--account "Name <email>"]  draws the rail as if the host were signed in as them
//!     [--scroll pixels]           scrolls the side rail down that far first
//!     [--files]                   shows the tab rail's Files page
//!     [--size 900x700]            draws the window at that size in points
//!     [--motion on|off]           animates or holds still whatever macOS's Reduce Motion says
//!     [--then step…]              then does each step in turn: "click x,y", "dclick x,y", "rclick x,y" or "move x,y" in points,
//!                                 "type text", "key cmd-enter", "wait seconds", "shot out.png", "scroll x,y pixels",
//!                                 "keys cmd-w cmd-w" to press several in one go, faster than the core answers,
//!                                 "swipe x,y pixels" or "swipe x,y pixels hold" for a sideways trackpad swipe,
//!                                 "drag x,y path…" or "drop x,y path…" to hold files over
//!                                 a place or drop them there, "copy-image file.png", or "share" to open the
//!                                 share dialog of the chat in front; "press x,y", "hold x,y" and
//!                                 "release x,y" drag the pointer with its button down, and
//!                                 "clipboard" prints the text copied; "state" prints one line of what
//!                                 the window shows (Settings, any modal, the front view, the focus, the tabs,
//!                                 the chosen host, a trust question, the front chat's tool details held and
//!                                 open); "activate" brings the window forward, as switching to the app does;
//!                                 "picker" prints the front chat's model menu, "choose-model <value>" picks a
//!                                 model in it and "move-to <provider>" its agent, as their rows do;
//!                                 "open-tool" opens the front chat's first tool call; "host this",
//!                                 "host host:<key>" or "host ssh:<alias>" chooses the host in front, as a
//!                                 click on its row does; "film N prefix" saves N frames at least 16 ms apart,
//!                                 as prefix-00.png on, straight after the step before it, printing when each
//!                                 was drawn; "menu name" prints the menus open, Kit's and the app's, as one line of
//!                                 JSON after "menu name: ", each a list of its items in order with their label
//!                                 and whether they are disabled or checked (null where the menu does not say,
//!                                 as Kit's items do not), and separators as {"separator":true};
//!                                 "toast success|info|error text" shows a toast saying the text;
//!                                 "settings <page>" opens settings at the page of that name

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    point, px, size, AppContext, ClipboardEntry, ClipboardItem, ExternalPaths, FileDropEvent,
    HeadlessAppContext, Image, ImageFormat, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, ScrollEvent, ScrollSource,
    TouchPhase,
};
use gpui_kit::component::WindowExt;
use serde_json::{json, Value};
use sikemux_gpui::{core, icons, theme, toast, workbench, WINDOW_SIZE};
use sikemux_wire::protocol::AccountInfo;

/// How often a wait lets the app run, moving its clock on as far, so its timers fire as they would.
const STEP: Duration = Duration::from_millis(50);
const FRAME: Duration = Duration::from_millis(16);

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "shot.png".into());
    let wait: f64 = std::env::args()
        .nth(2)
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or(3.);
    let option = |name: &str| {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1).cloned())
    };
    let account = option("--account");
    let window_size = option("--size")
        .and_then(|size| {
            let (width, height) = size.split_once('x')?;
            Some((width.parse().ok()?, height.parse().ok()?))
        })
        .unwrap_or(WINDOW_SIZE);
    let files = std::env::args().any(|arg| arg == "--files");
    let motion = option("--motion").map(|motion| motion == "on");
    let scroll: Option<f32> = option("--scroll").and_then(|pixels| pixels.parse().ok());
    let steps: Vec<String> = std::env::args()
        .skip_while(|arg| arg != "--then")
        .skip(1)
        .collect();
    let bench = Rc::new(RefCell::new(None));
    let kept = bench.clone();
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    let link = core::start(core::Home::default());
    let window = cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        let link = link;
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                    origin: gpui::point(px(0.), px(0.)),
                    size: size(px(window_size.0), px(window_size.1)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| workbench::Workbench::new(link, window, cx));
                *kept.borrow_mut() = Some(view.clone());
                cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
            },
        )
    })?;
    // Offscreen, the window never comes to the front, which is when the app takes the core's
    // calls, so it takes them from the start.
    if let Some(bench) = bench.borrow().clone() {
        cx.update(|cx| {
            let _ = bench.read(cx).asks.send(core::Ask::RegisterWindow);
        });
    }
    if files {
        if let Some(bench) = bench.borrow().clone() {
            cx.update(|cx| {
                bench.update(cx, |bench, cx| {
                    bench.rails.page = workbench::RailPage::Files;
                    cx.notify();
                })
            });
        }
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs_f64(wait) {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
    }
    if let (Some(account), Some(bench)) = (account, bench.borrow().clone()) {
        let (name, email) = account
            .split_once(" <")
            .map_or((account.as_str(), ""), |(name, email)| {
                (name, email.trim_end_matches('>'))
            });
        let shown = AccountInfo {
            name: Some(name.into()),
            email: Some(email.into()),
            picture_hash: None,
        };
        cx.update(|cx| {
            bench.update(cx, |bench, cx| {
                if let Some(view) = bench.view.as_mut() {
                    view.account = Some(Box::new(shown));
                }
                cx.notify();
            })
        });
    }
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    if let Some(pixels) = scroll {
        cx.update_window(window.into(), |_, window, cx| {
            let wheel = ScrollEvent {
                position: point(px(120.), px(400.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-pixels))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                source: ScrollSource::Pointer,
            };
            window.dispatch_event(PlatformInput::Scroll(wheel), cx);
        })?;
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    }
    cx.capture_screenshot(window.into())?.save(&out)?;
    // The app takes macOS's setting again as its window shows or comes forward, so this wins
    // before every step and frame.
    let hold_motion = |cx: &mut HeadlessAppContext| {
        if let Some(motion) = motion {
            cx.update(|cx| cx.set_reduce_motion(!motion));
        }
    };
    for (at, step) in steps.iter().enumerate() {
        hold_motion(&mut cx);
        let (verb, rest) = step.split_once(' ').unwrap_or((step.as_str(), ""));
        cx.update_window(window.into(), |_, window, cx| -> anyhow::Result<()> {
            match verb {
                "move" => {
                    let (x, y) = rest
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("move x,y"))?;
                    let position = point(px(x.trim().parse()?), px(y.trim().parse()?));
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                    window.draw(cx).clear(cx);
                }
                "click" | "dclick" | "rclick" => {
                    let (x, y) = rest
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("{verb} x,y"))?;
                    let position = point(px(x.trim().parse()?), px(y.trim().parse()?));
                    let modifiers = Modifiers::default();
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers,
                        }),
                        cx,
                    );
                    window.draw(cx).clear(cx);
                    let button = if verb == "rclick" {
                        MouseButton::Right
                    } else {
                        MouseButton::Left
                    };
                    let click_count = if verb == "dclick" { 2 } else { 1 };
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button,
                            position,
                            modifiers,
                            click_count,
                            first_mouse: false,
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button,
                            position,
                            modifiers,
                            click_count,
                        }),
                        cx,
                    );
                }
                "press" | "hold" | "release" => {
                    let (x, y) = rest
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("{verb} x,y"))?;
                    let position = point(px(x.trim().parse()?), px(y.trim().parse()?));
                    let (button, modifiers) = (MouseButton::Left, Modifiers::default());
                    let event = match verb {
                        "press" => PlatformInput::MouseDown(MouseDownEvent {
                            button,
                            position,
                            modifiers,
                            click_count: 1,
                            first_mouse: false,
                        }),
                        "hold" => PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: Some(button),
                            modifiers,
                        }),
                        _ => PlatformInput::MouseUp(MouseUpEvent {
                            button,
                            position,
                            modifiers,
                            click_count: 1,
                        }),
                    };
                    window.dispatch_event(event, cx);
                    window.draw(cx).clear(cx);
                }
                "clipboard" => {
                    let copied = cx.read_from_clipboard().and_then(|item| item.text());
                    println!("clipboard: {copied:?}");
                }
                "state" => {
                    if let Some(bench) = bench.borrow().clone() {
                        let (held, open) = bench.read(cx).tool_details(cx).unwrap_or_default();
                        let (strip, composer) = cx
                            .try_global::<sikemux_gpui::composer::Drawn>()
                            .map_or((0., 0.), |drawn| (drawn.strip, drawn.composer));
                        println!(
                            "{} held={held} open={open} strip={strip:.0} composer={composer:.0}",
                            state(bench.read(cx), &window.context_stack(), cx)
                        );
                    }
                }
                "menu" => println!("menu {rest}: {}", Value::Array(open_menus(window))),
                "toast" => {
                    let (kind, said) = rest.split_once(' ').unwrap_or((rest, ""));
                    let said = said.to_owned();
                    let note = match kind {
                        "error" => toast::error(said),
                        "info" => toast::info(said),
                        _ => toast::success(said),
                    };
                    window.push_notification(note, cx);
                }
                "activate" => window.activate_window(),
                "settings" => {
                    let page = sikemux_gpui::settings::Page::named(rest.trim())
                        .ok_or_else(|| anyhow::anyhow!("settings <page name>"))?;
                    if let Some(bench) = bench.borrow().clone() {
                        bench.update(cx, |bench, cx| {
                            sikemux_gpui::settings::open(bench, page, window, cx)
                        });
                    }
                    window.draw(cx).clear(cx);
                }
                "picker" => {
                    if let Some(bench) = bench.borrow().clone() {
                        match bench.read(cx).front_models() {
                            Some((current, offered)) => {
                                println!("picker current={current} offered={}", offered.join(","))
                            }
                            None => println!("picker none"),
                        }
                    }
                }
                "choose-model" => {
                    if let Some(bench) = bench.borrow().clone() {
                        bench.read(cx).choose_front_model(rest);
                    }
                }
                "move-to" => {
                    if let Some(bench) = bench.borrow().clone() {
                        bench.update(cx, |bench, _| bench.move_front_to(rest));
                    }
                }
                "open-tool" => {
                    if let Some(bench) = bench.borrow().clone() {
                        bench.update(cx, |bench, cx| bench.open_first_tool(cx));
                    }
                }
                "host" => {
                    let choice = match rest.split_once(':') {
                        Some(("host", key)) => sikemux_gpui::hosts::HostChoice::Account(key.into()),
                        Some(("ssh", alias)) => sikemux_gpui::hosts::HostChoice::Ssh(alias.into()),
                        _ => sikemux_gpui::hosts::HostChoice::This,
                    };
                    if let Some(bench) = bench.borrow().clone() {
                        bench.update(cx, |bench, cx| bench.choose_host(choice, cx));
                    }
                }
                "type" => {
                    for letter in rest.chars() {
                        let key = if letter == ' ' {
                            "space".to_owned()
                        } else {
                            letter.to_string()
                        };
                        window.dispatch_keystroke(
                            Keystroke::parse_with_os(&key, cx.platform_info().os)?,
                            cx,
                        );
                    }
                }
                "key" => {
                    window.dispatch_keystroke(
                        Keystroke::parse_with_os(rest, cx.platform_info().os)?,
                        cx,
                    );
                }
                "keys" => {
                    for key in rest.split_whitespace() {
                        window.dispatch_keystroke(
                            Keystroke::parse_with_os(key, cx.platform_info().os)?,
                            cx,
                        );
                    }
                }
                "scroll" => {
                    let (at, pixels) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("scroll x,y pixels"))?;
                    let (x, y) = at
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("scroll x,y pixels"))?;
                    let wheel = ScrollEvent {
                        position: point(px(x.trim().parse()?), px(y.trim().parse()?)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(pixels.trim().parse()?))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    };
                    window.dispatch_event(PlatformInput::Scroll(wheel), cx);
                }
                "swipe" => {
                    let mut words = rest.split(' ');
                    let (x, y) = words
                        .next()
                        .and_then(|at| at.split_once(','))
                        .ok_or_else(|| anyhow::anyhow!("swipe x,y pixels"))?;
                    let pixels: f32 = words
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("swipe x,y pixels"))?
                        .parse()?;
                    let hold = words.next() == Some("hold");
                    let position = point(px(x.trim().parse()?), px(y.trim().parse()?));
                    let wheel = |dx: f32, touch_phase| ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
                        modifiers: Modifiers::default(),
                        touch_phase,
                        source: ScrollSource::Pointer,
                    };
                    window
                        .dispatch_event(PlatformInput::Scroll(wheel(0., TouchPhase::Started)), cx);
                    for _ in 0..10 {
                        window.dispatch_event(
                            PlatformInput::Scroll(wheel(pixels / 10., TouchPhase::Moved)),
                            cx,
                        );
                        std::thread::sleep(Duration::from_millis(8));
                    }
                    if !hold {
                        window.dispatch_event(
                            PlatformInput::Scroll(wheel(0., TouchPhase::Ended)),
                            cx,
                        );
                    }
                    window.draw(cx).clear(cx);
                }
                "drag" | "drop" => {
                    let (at, paths) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("{verb} x,y path…"))?;
                    let (x, y) = at
                        .split_once(',')
                        .ok_or_else(|| anyhow::anyhow!("{verb} x,y path…"))?;
                    let position = point(px(x.trim().parse()?), px(y.trim().parse()?));
                    let paths = ExternalPaths(paths.split(' ').map(PathBuf::from).collect());
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }),
                        cx,
                    );
                    window.draw(cx).clear(cx);
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Pending { position }),
                        cx,
                    );
                    if verb == "drop" {
                        window.draw(cx).clear(cx);
                        window.dispatch_event(
                            PlatformInput::FileDrop(FileDropEvent::Submit { position }),
                            cx,
                        );
                    }
                }
                "share" => {
                    if let Some(bench) = bench.borrow().clone() {
                        bench.update(cx, |bench, cx| {
                            if let Some(agent) = bench.agent() {
                                bench.open_share(&agent.id, window, cx);
                            }
                        });
                    }
                }
                "copy-image" => {
                    let image = Image::from_bytes(ImageFormat::Png, std::fs::read(rest)?);
                    cx.write_to_clipboard(ClipboardItem {
                        entries: vec![ClipboardEntry::Image(image)],
                    });
                }
                _ => {}
            }
            Ok(())
        })??;
        if verb == "film" {
            let (count, prefix) = rest
                .split_once(' ')
                .ok_or_else(|| anyhow::anyhow!("film N prefix"))?;
            let began = Instant::now();
            for frame in 0..count.trim().parse::<usize>()? {
                hold_motion(&mut cx);
                std::thread::sleep(FRAME);
                cx.advance_clock(FRAME);
                cx.run_until_parked();
                // Offscreen there is no display link to run what asked for the next frame.
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                })?;
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
                let drawn = began.elapsed().as_millis();
                cx.capture_screenshot(window.into())?
                    .save(format!("{prefix}-{frame:02}.png"))?;
                println!("film {prefix}-{frame:02}.png drawn {drawn} ms in");
            }
            continue;
        }
        let filmed = steps
            .get(at + 1)
            .is_some_and(|next| next.starts_with("film "));
        let pause = match verb {
            "wait" => rest.parse().unwrap_or(1.),
            _ if filmed => 0.,
            _ => 0.3,
        };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs_f64(pause) {
            cx.run_until_parked();
            std::thread::sleep(STEP);
            cx.advance_clock(STEP);
        }
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
        if verb == "shot" {
            cx.capture_screenshot(window.into())?.save(rest)?;
        }
    }
    Ok(())
}

/// One line of what the window shows, for a check to match without looking: its keys stay in
/// this order, and new ones go at the end.
/// The menus the last frame drew, each as its items in order. An item takes its own accessible label
/// or, for an item drawn from its own element, the first label inside it; an item with no label
/// anywhere is a separator when it is a thin line, as both Kit's menus and the app's draw them.
fn open_menus(window: &gpui::Window) -> Vec<Value> {
    let mut drawn = gpui_base::test_support::snapshots(window);
    drawn.sort_by(|a, b| {
        let at = |e: &gpui_base::test_support::ElementSnapshot| {
            (e.bounds().origin.y, e.bounds().origin.x)
        };
        at(a)
            .partial_cmp(&at(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let inside = |outer: &[gpui::ElementId], inner: &[gpui::ElementId]| {
        inner.len() > outer.len() && inner.starts_with(outer)
    };
    drawn
        .iter()
        .filter(|element| element.role() == Some(gpui::Role::Menu))
        .map(|menu| {
            let candidates: Vec<_> = drawn
                .iter()
                .filter(|element| {
                    inside(menu.path(), element.path())
                        && matches!(element.role(), Some(gpui::Role::MenuItem) | None)
                })
                .collect();
            let items = candidates
                .iter()
                .filter(|item| {
                    !candidates
                        .iter()
                        .any(|outer| inside(outer.path(), item.path()))
                })
                .map(|item| {
                    let label = item.label().or_else(|| {
                        drawn
                            .iter()
                            .find(|element| {
                                inside(item.path(), element.path()) && element.label().is_some()
                            })
                            .and_then(|element| element.label())
                    });
                    match label {
                        Some(label) => json!({
                            "label": label,
                            "disabled": item.disabled(),
                            "checked": item.checked(),
                        }),
                        None if item.bounds().size.height < px(8.) => json!({ "separator": true }),
                        None => {
                            json!({ "label": null, "disabled": item.disabled() })
                        }
                    }
                })
                .collect();
            Value::Array(items)
        })
        .collect()
}

fn state(bench: &workbench::Workbench, stack: &[gpui::KeyContext], cx: &gpui::App) -> String {
    let names = |context: &gpui::KeyContext| -> Vec<String> {
        context
            .primary()
            .into_iter()
            .chain(context.secondary())
            .filter(|entry| entry.value.is_none())
            .map(|entry| entry.key.to_string())
            .collect()
    };
    let modal = stack
        .iter()
        .rev()
        .find(|context| context.contains("Modal"))
        .and_then(|context| names(context).into_iter().find(|name| name != "Modal"));
    let focus = stack
        .last()
        .and_then(|context| names(context).into_iter().next());
    let tab = bench.focused_kind().map(|kind| format!("{kind:?}"));
    let tabs = bench.strip().map_or(0, |strip| bench.tabs(&strip).len());
    let settings = if bench.settings.read(cx).is_open() {
        "open"
    } else {
        "closed"
    };
    let shown = |name: Option<String>| name.map_or_else(|| "none".to_owned(), |name| kebab(&name));
    let host = bench.chosen_host().key();
    let asking = !bench.reads.trust.is_empty() || !bench.reads.host_trust.is_empty();
    let trust = if asking { "asking" } else { "none" };
    format!(
        "state settings={settings} modal={} tab={} focus={} tabs={tabs} host={host} trust={trust}",
        shown(modal),
        shown(tab),
        shown(focus),
    )
}

/// `ProjectSwitcher` as `project-switcher`.
fn kebab(name: &str) -> String {
    let mut kebab = String::new();
    for (at, letter) in name.chars().enumerate() {
        if letter.is_uppercase() && at > 0 {
            kebab.push('-');
        }
        kebab.extend(letter.to_lowercase());
    }
    kebab
}
