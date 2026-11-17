//! Under XWayland, GPUI cannot see the scale the Wayland compositor gives each display, and
//! guesses one from a display's physical size. This asks the compositor instead, and keeps
//! the window at the scale of the display it is on as displays change and the window moves.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{Context, Window};
use sikemux_process::sync::Locked as _;
use tokio::sync::mpsc::{self, UnboundedSender};
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use x11rb::connection::Connection as _;
use x11rb::protocol::randr::{ConnectionExt as _, NotifyMask};
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
use x11rb::rust_connection::RustConnection;

const SCALE_VARIABLE: &str = "GPUI_X11_SCALE_FACTOR";

static CHOSE_SCALE: AtomicBool = AtomicBool::new(false);

/// Sets the scale the window opens at, the lowest of the displays because which one it will
/// open on is not known yet. Leaves the scale alone when the person set one, the desktop
/// published a DPI, or this is not a Wayland session. It sets the environment, so it runs
/// before any thread starts.
pub fn choose() {
    let is_set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if !is_set("WAYLAND_DISPLAY") || !is_set("DISPLAY") || is_set(SCALE_VARIABLE) {
        return;
    }
    let Ok((x11, screen)) = x11rb::connect(None) else {
        return;
    };
    if desktop_published_dpi(&x11) {
        return;
    }
    let Some(outputs) = wayland_outputs() else {
        return;
    };
    let scale = x11_monitors(&x11, screen)
        .unwrap_or_default()
        .iter()
        .filter_map(|monitor| monitor.scale_on(&outputs))
        .reduce(f32::min)
        .unwrap_or(1.0);
    std::env::set_var(SCALE_VARIABLE, scale.to_string());
    CHOSE_SCALE.store(true, Ordering::Relaxed);
    eprintln!("sikemux: XWayland windows open at {scale}x");
}

/// Keeps `window` at the scale of the display it is on, when [`choose`] set its scale.
pub fn follow<V: 'static>(window: &mut Window, cx: &mut Context<V>) {
    if !CHOSE_SCALE.load(Ordering::Relaxed) {
        return;
    }
    let Some(x11_window) = gpui_native_view::window_x11(window) else {
        return;
    };
    let (changed, mut changes) = mpsc::unbounded_channel();
    let watch = Watch {
        displays: Arc::default(),
        changed,
    };
    let wayland_watch = watch.clone();
    let _ = std::thread::Builder::new()
        .name("scale-wayland".into())
        .spawn(move || watch_wayland(&wayland_watch));
    let _ = std::thread::Builder::new()
        .name("scale-x11".into())
        .spawn(move || watch_x11(&watch, x11_window));
    cx.spawn_in(window, async move |_, cx| {
        while let Some(scale) = changes.recv().await {
            let applied = cx.update(|window, cx| {
                if window.scale_factor() != scale {
                    window.set_platform_scale_factor(scale, cx);
                }
            });
            if applied.is_err() {
                break;
            }
        }
    })
    .detach();
}

fn desktop_published_dpi(x11: &RustConnection) -> bool {
    x11rb::resource_manager::new_from_default(x11)
        .ok()
        .and_then(|database| database.get_value::<f32>("Xft.dpi", "Xft.dpi").ok())
        .flatten()
        .is_some()
}

#[derive(Clone)]
struct Watch {
    displays: Arc<Mutex<Displays>>,
    changed: UnboundedSender<f32>,
}

impl Watch {
    fn update(&self, change: impl FnOnce(&mut Displays)) {
        let mut displays = self.displays.locked();
        change(&mut displays);
        let Some(scale) = displays.window_scale() else {
            return;
        };
        if displays.last_scale != Some(scale) {
            displays.last_scale = Some(scale);
            let _ = self.changed.send(scale);
        }
    }
}

#[derive(Default)]
struct Displays {
    outputs: Vec<Output>,
    monitors: Vec<X11Monitor>,
    window_centre: Option<(i32, i32)>,
    last_scale: Option<f32>,
}

impl Displays {
    fn window_scale(&self) -> Option<f32> {
        let centre = self.window_centre?;
        self.monitors
            .iter()
            .find(|monitor| monitor.contains(centre))?
            .scale_on(&self.outputs)
    }
}

fn watch_x11(watch: &Watch, window: u32) -> Option<()> {
    let (x11, screen) = x11rb::connect(None).ok()?;
    let root = x11.setup().roots.get(screen)?.root;
    x11.randr_query_version(1, 6).ok()?.reply().ok()?;
    let displays_change =
        NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE;
    x11.randr_select_input(root, displays_change).ok()?;
    let moves = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
    x11.change_window_attributes(window, &moves).ok()?;
    loop {
        let monitors = x11_monitors(&x11, screen)?;
        let centre = window_centre(&x11, root, window)?;
        watch.update(|displays| {
            displays.monitors = monitors;
            displays.window_centre = Some(centre);
        });
        x11.wait_for_event().ok()?;
        while x11.poll_for_event().ok()?.is_some() {}
    }
}

fn window_centre(x11: &RustConnection, root: u32, window: u32) -> Option<(i32, i32)> {
    let size = x11.get_geometry(window).ok()?.reply().ok()?;
    let centre = x11
        .translate_coordinates(
            window,
            root,
            (size.width / 2) as i16,
            (size.height / 2) as i16,
        )
        .ok()?
        .reply()
        .ok()?;
    Some((i32::from(centre.dst_x), i32::from(centre.dst_y)))
}

fn watch_wayland(watch: &Watch) -> Option<()> {
    let (mut outputs, mut queue, _connection) = connect_wayland()?;
    loop {
        queue.blocking_dispatch(&mut outputs).ok()?;
        let current = outputs.list.clone();
        watch.update(|displays| displays.outputs = current);
    }
}

struct X11Monitor {
    name: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl X11Monitor {
    fn contains(&self, (x, y): (i32, i32)) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }

    fn scale_on(&self, outputs: &[Output]) -> Option<f32> {
        let output = outputs.iter().find(|output| output.name == self.name)?;
        Some(output.scale_for_x11(self.width.max(self.height)))
    }
}

fn x11_monitors(x11: &RustConnection, screen: usize) -> Option<Vec<X11Monitor>> {
    let root = x11.setup().roots.get(screen)?.root;
    x11.randr_query_version(1, 6).ok()?.reply().ok()?;
    let reply = x11.randr_get_monitors(root, true).ok()?.reply().ok()?;
    reply
        .monitors
        .iter()
        .map(|monitor| {
            let name = x11.get_atom_name(monitor.name).ok()?.reply().ok()?.name;
            Some(X11Monitor {
                name: String::from_utf8_lossy(&name).into_owned(),
                x: i32::from(monitor.x),
                y: i32::from(monitor.y),
                width: i32::from(monitor.width),
                height: i32::from(monitor.height),
            })
        })
        .collect()
}

/// Sides are the longer of width and height, so a rotated display compares the same.
#[derive(Clone, Default)]
struct Output {
    global: u32,
    name: String,
    mode_long_side: i32,
    logical_long_side: i32,
}

impl Output {
    /// A compositor either gives X11 windows the display's real pixels, and they must draw at
    /// its scale themselves, or gives them logical pixels and enlarges them itself.
    fn scale_for_x11(&self, x11_long_side: i32) -> f32 {
        if x11_long_side != self.mode_long_side || self.logical_long_side <= 0 {
            return 1.0;
        }
        let scale = self.mode_long_side as f32 / self.logical_long_side as f32;
        (scale * 120.0).round() / 120.0
    }
}

#[derive(Default)]
struct Outputs {
    list: Vec<Output>,
    manager: Option<ZxdgOutputManagerV1>,
}

impl Outputs {
    fn bind(
        &mut self,
        registry: &WlRegistry,
        global: u32,
        version: u32,
        handle: &QueueHandle<Self>,
    ) {
        self.list.push(Output {
            global,
            ..Output::default()
        });
        let output: WlOutput = registry.bind(global, version.min(4), handle, global);
        if let Some(manager) = &self.manager {
            manager.get_xdg_output(&output, handle, global);
        }
    }

    fn with_global(&mut self, global: u32, change: impl FnOnce(&mut Output)) {
        if let Some(output) = self.list.iter_mut().find(|output| output.global == global) {
            change(output);
        }
    }
}

fn connect_wayland() -> Option<(Outputs, wayland_client::EventQueue<Outputs>, Connection)> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, queue): (GlobalList, _) = registry_queue_init(&connection).ok()?;
    let handle = queue.handle();
    let mut outputs = Outputs {
        list: Vec::new(),
        manager: globals.bind(&handle, 2..=3, ()).ok(),
    };
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            outputs.bind(globals.registry(), global.name, global.version, &handle);
        }
    }
    Some((outputs, queue, connection))
}

fn wayland_outputs() -> Option<Vec<Output>> {
    let (mut outputs, mut queue, _connection) = connect_wayland()?;
    queue.roundtrip(&mut outputs).ok()?;
    Some(outputs.list)
}

impl Dispatch<WlRegistry, GlobalListContents> for Outputs {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.bind(registry, name, version, handle),
            wl_registry::Event::GlobalRemove { name } => {
                state.list.retain(|output| output.global != name);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for Outputs {
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: <ZxdgOutputManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, u32> for Outputs {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                state.with_global(*global, |output| {
                    output.mode_long_side = width.max(height);
                });
            }
            wl_output::Event::Name { name } => {
                state.with_global(*global, |output| output.name = name);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for Outputs {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                state.with_global(*global, |output| {
                    output.logical_long_side = width.max(height);
                });
            }
            zxdg_output_v1::Event::Name { name } => {
                state.with_global(*global, |output| output.name = name);
            }
            _ => {}
        }
    }
}
