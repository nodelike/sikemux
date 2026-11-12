//! Films the app for posts, the way the launch reel was made: a scene moves a drawn cursor and a
//! camera over the real window, drawn offscreen a sixtieth of a second at a time, while a
//! throwaway core and a stand-in Claude answer it. `film/film.sh` builds what it needs and runs
//! it; scenes live in `film/scenes`.
//!   cargo run --example film -- <scene.film> --core <sikemux-host> --claude <sikemux-fake-claude>
//!     --work <folder> --out <clip.mp4, or a folder for stills> [--events <file.json>]
//!     [--wallpaper <png>] [--lockup <png>] [--height 1440] [--blur] [--draft]
//!     [--stills 1.5,4] [--grid]
//!
//! The window's native views, the desk's browser and the pages agents show, live in a stand-in
//! window offscreen, and each picture of the window has them laid over it where they sit.
//!
//! A scene is a step a line, `#` starting a comment; a step ending in `&` plays on while the
//! next ones start. A place is in window points: `x,y`, a box `x,y,w,h`, or the quoted label of
//! a control the window reports. Times are in seconds.
//!   stage W H / window W H      the frame and the window in its middle, in points
//!   replies FILE                the stand-in Claude's scripted replies (see sikemux-fake-claude)
//!   project NAME FOLDER         copies FOLDER, beside the scene, into a new git project
//!   chat NAME                   opens a Claude chat named NAME in the last project, first in its strip
//!   title "TEXT"                gives the last chat that title
//!   trigger "TITLE" SECONDS     sets the last chat a trigger that sends it TITLE every SECONDS
//!   say "TEXT"                  sends the last chat TEXT, as the person, with no typing shown
//!   pause ID                    pauses trigger ID as the person does
//!   auto SLOTS DOLLARS          turns Auto on in the last project, SLOTS at once, up to DOLLARS
//!   page ID FILE                keeps FILE as the page a scripted page_show call names by ID
//!   open FILE|terminal|git      opens FILE of the last project, a new terminal or Git in a tab
//!   patch FILE                  applies FILE, a diff beside the scene, to the last project unstaged
//!   prefer KEY [VALUE]          sets a preference, VALUE as JSON or else as text, or puts
//!                               back the world's
//!   settle S                    lets the core and the window catch up, unfilmed
//!   hold S                      gives the core S real seconds with the film's time stopped, so
//!                               an answer that takes a round trip lands between two frames
//!   record                      starts filming; the steps before it play unseen
//!   cursor P / move P [S]       puts the cursor at P, or glides it there
//!   click [P] / dclick [P] / rclick [P]    clicks, gliding to P first
//!   press / release / drag P [S]           holds the button, lets it go, or drags to P
//!   type "TEXT" [PER_SECOND]    types at a person's pace
//!   key NAME                    presses a key such as enter or cmd-k
//!   zoom P SCALE [S] / zoom BOX [S] / zoom out [S]    moves the camera
//!   pinch FACTOR [S]            pinches at the cursor, above 1 to zoom in
//!   pan DX,DY [S] / scroll DY [S]          two-finger scrolls at the cursor
//!   shape arrow|hand|beam|auto  which pointer is drawn
//!   settings PAGE               opens settings at the page of that name
//!   wait S / mark KIND / still NAME / end [HOLD]
//!   snap NAME                   saves the window alone as drawn, without the stage or cursor
//!   calls FILE                  puts the calls FILE lists, as the core's view writes them, to
//!                               the person: each asked by the agent of its asker's name, in the
//!                               project of its project's name, raised its raisedAt ms ago

mod director;
#[cfg(target_os = "macos")]
mod native;
mod scene;
mod world;

use director::*;
use scene::*;
use sikemux_wire::protocol::ask;
use std::cell::RefCell;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use world::*;

use anyhow::{anyhow, bail, Context as _};
use film_frame::{Compositor, Picture, Pointer, Setup, Shape, View};
use gpui::{
    point, px, size, AnyWindowHandle, AppContext, HeadlessAppContext, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, PlatformInput, Role,
    ScrollDelta, ScrollEvent, ScrollSource, TouchPhase,
};
use serde_json::{json, Value};
use sikemux_client::client::CoreClient;
use sikemux_gpui::{core, icons, theme, workbench};

const FPS: f64 = 60.;
/// The model the stand-in Claude and the seeded year of activity say did the work.
const SEEDED_MODEL: &str = "claude-opus-4-5";

struct Args {
    scene: PathBuf,
    core: PathBuf,
    claude: PathBuf,
    work: PathBuf,
    out: PathBuf,
    events: Option<PathBuf>,
    wallpaper: Option<PathBuf>,
    lockup: Option<PathBuf>,
    height: u32,
    blur: bool,
    draft: bool,
    stills: Option<Vec<f64>>,
    grid: bool,
    world: Value,
}

fn args() -> anyhow::Result<Args> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        all.iter()
            .position(|arg| arg == name)
            .and_then(|at| all.get(at + 1).cloned())
    };
    let flag = |name: &str| all.iter().any(|arg| arg == name);
    let path = |name: &str| {
        option(name)
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("{name} is needed"))
    };
    let draft = flag("--draft");
    Ok(Args {
        scene: all
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("which scene?"))?,
        core: path("--core")?,
        claude: path("--claude")?,
        work: path("--work")?,
        out: path("--out")?,
        events: option("--events").map(PathBuf::from),
        wallpaper: option("--wallpaper").map(PathBuf::from),
        lockup: option("--lockup").map(PathBuf::from),
        height: option("--height")
            .and_then(|height| height.parse().ok())
            .unwrap_or(if draft { 1080 } else { 1440 }),
        blur: flag("--blur"),
        draft,
        stills: option("--stills").map(|list| {
            list.split(',')
                .filter_map(|seconds| seconds.trim().parse().ok())
                .collect()
        }),
        grid: flag("--grid"),
        world: match option("--world") {
            Some(path) => serde_json::from_str(&std::fs::read_to_string(&path)?)
                .with_context(|| format!("reading {path}"))?,
            None => Value::Null,
        },
    })
}

fn main() -> anyhow::Result<()> {
    let args = args()?;
    let scene_text = std::fs::read_to_string(&args.scene)
        .with_context(|| format!("reading {}", args.scene.display()))?;
    let steps = read_scene(&scene_text)?;
    let header = |verb: &str| steps.iter().find(|step| step.verb == verb);
    let pair = |verb: &str, fallback: (f32, f32)| {
        header(verb)
            .and_then(|step| Some((step.number(0)?, step.number(1)?)))
            .unwrap_or(fallback)
    };
    let stage = pair("stage", (1760., 990.));
    let window_size = pair("window", (1520., 900.));
    let replies = header("replies")
        .map(|step| step.text(0).map(|file| args.scene.with_file_name(file)))
        .transpose()?;
    std::fs::create_dir_all(&args.work)?;
    let clock = args.work.join("clock");
    std::fs::write(&clock, "0")?;
    let mut world = World::start(&args, replies, &clock)?;

    let bench = Rc::new(RefCell::new(None));
    let kept = bench.clone();
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    #[cfg(target_os = "macos")]
    let stand_in = native::StandIn::open(window_size);
    let link = core::start(core::Home::at(world.socket.clone()));
    let window = cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.set_reduce_motion(false);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                    origin: point(px(0.), px(0.)),
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
    // A GIF plays only in the window in front, as on the Mac.
    cx.update_window(window.into(), |_, window, _| window.activate_window())?;
    if let Some(bench) = bench.borrow().clone() {
        cx.update(|cx| {
            let _ = bench.read(cx).asks.send(core::Ask::RegisterWindow);
        });
    }
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
    let density = cx.capture_screenshot(window.into())?.width() as f32 / window_size.0;

    let output_height = args.height;
    let output_width = ((output_height as f32 * stage.0 / stage.1 / 2.).round() * 2.) as u32;
    let compositor = Compositor::new(Setup {
        stage,
        window: window_size,
        density,
        output: (output_width, output_height),
        wallpaper: args.wallpaper.as_deref(),
        lockup: args.lockup.as_deref(),
    });
    let origin = compositor.window_origin();
    let (sink, stills) = match &args.stills {
        Some(times) => {
            std::fs::create_dir_all(&args.out)?;
            let frames = times
                .iter()
                .map(|seconds| (seconds * FPS).round() as u64)
                .collect();
            (Sink::Stills { frames }, args.out.clone())
        }
        None => {
            let partial = args.out.with_extension("partial.mp4");
            let mut ffmpeg = sikemux_process::user_environment::command("ffmpeg")
                .args([
                    "-y",
                    "-loglevel",
                    "error",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                ])
                .args(["-s", &format!("{output_width}x{output_height}")])
                .args(["-framerate", &FPS.to_string(), "-i", "-"])
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    if args.draft { "veryfast" } else { "slow" },
                ])
                .args([
                    "-crf",
                    if args.draft { "23" } else { "14" },
                    "-pix_fmt",
                    "yuv420p",
                ])
                .args(["-profile:v", "high", "-movflags", "+faststart"])
                .arg(&partial)
                .stdin(Stdio::piped())
                .spawn()
                .context("starting ffmpeg")?;
            let input = ffmpeg.stdin.take();
            let folder = args
                .out
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            (
                Sink::Video {
                    ffmpeg,
                    input,
                    partial,
                    out: args.out.clone(),
                },
                folder,
            )
        }
    };
    let mut director = Director {
        cx,
        window: window.into(),
        stage,
        origin,
        pictures_a_frame: if args.blur { 2 } else { 1 },
        ticks: 0,
        waited_ms: 0.,
        clock,
        recording: false,
        recorded: 0,
        pointer: Pointer {
            x: window_size.0 * 0.75,
            y: window_size.1 * 0.8,
            press: 0.,
            opacity: 1.,
            shape: Shape::Arrow,
        },
        held: false,
        sent: None,
        forced: None,
        view: View {
            x: 0.,
            y: 0.,
            width: stage.0,
        },
        end: 0.,
        tweens: Vec::new(),
        next_id: 0,
        lens: None,
        bench: bench.borrow().clone(),
        #[cfg(target_os = "macos")]
        stand_in,
        calls: Vec::new(),
        signed_in: args.world["signedIn"]["name"].as_str().map(|name| {
            sikemux_view::device::AccountInfo {
                name: Some(name.into()),
                email: args.world["signedIn"]["email"].as_str().map(Into::into),
                picture_hash: None,
            }
        }),
        compositor,
        output: (output_width, output_height),
        sink,
        still: None,
        snap: None,
        stills,
        grid: args.grid,
        events: Vec::new(),
        seed: 7,
        began: Instant::now(),
        spent: [Duration::ZERO; 4],
    };
    let mut played = Ok(());
    for step in &steps {
        let taken = match world.take(step) {
            Ok(taken) => taken,
            Err(error) => {
                played = Err(error.context(format!("line {}", step.line)));
                break;
            }
        };
        if taken {
            continue;
        }
        if let Err(error) = director.run(step) {
            played = Err(error);
            break;
        }
    }
    while director.recording && !director.tweens.is_empty() && played.is_ok() {
        played = director.frame();
    }
    drop(world);
    let played = match played {
        Err(error) if error.is::<Finished>() => Ok(()),
        other => other,
    };
    if let Sink::Video {
        mut ffmpeg,
        input,
        partial,
        out,
    } = std::mem::replace(&mut director.sink, Sink::Stills { frames: Vec::new() })
    {
        drop(input);
        let encoded = ffmpeg.wait()?;
        if played.is_ok() && encoded.success() {
            std::fs::rename(&partial, &out)?;
            println!(
                "film: wrote {} ({:.1} s)",
                out.display(),
                director.recorded as f64 / FPS
            );
        }
    }
    if let Some(events) = &args.events {
        std::fs::write(
            events,
            serde_json::to_string_pretty(&json!({
                "fps": FPS,
                "seconds": director.recorded as f64 / FPS,
                "events": director.events,
            }))?,
        )?;
    }
    played
}
