//! Under XWayland, GPUI cannot see the scale the Wayland compositor gives each display, and
//! guesses one from a display's physical size. This asks the compositor instead.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use x11rb::connection::Connection as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

const SCALE_VARIABLE: &str = "GPUI_X11_SCALE_FACTOR";

/// Leaves the scale alone when the person set one, the desktop published a DPI, or this is not
/// a Wayland session. It sets the environment, so it runs before any thread starts.
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
        .filter_map(|monitor| {
            let output = outputs.iter().find(|output| output.name == monitor.name)?;
            Some(output.scale_for_x11(monitor.long_side))
        })
        .reduce(f32::min)
        .unwrap_or(1.0);
    std::env::set_var(SCALE_VARIABLE, scale.to_string());
    eprintln!("sikemux: XWayland windows draw at {scale}x");
}

fn desktop_published_dpi(x11: &RustConnection) -> bool {
    x11rb::resource_manager::new_from_default(x11)
        .ok()
        .and_then(|database| database.get_value::<f32>("Xft.dpi", "Xft.dpi").ok())
        .flatten()
        .is_some()
}

struct X11Monitor {
    name: String,
    long_side: i32,
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
                long_side: i32::from(monitor.width.max(monitor.height)),
            })
        })
        .collect()
}

/// Sides are the longer of width and height, so a rotated display compares the same.
#[derive(Default)]
struct Output {
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
struct Outputs(Vec<Output>);

fn wayland_outputs() -> Option<Vec<Output>> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<Outputs>(&connection).ok()?;
    let handle = queue.handle();
    let manager: Option<ZxdgOutputManagerV1> = globals.bind(&handle, 2..=3, ()).ok();
    let mut outputs = Outputs::default();
    for global in globals.contents().clone_list() {
        if global.interface != "wl_output" {
            continue;
        }
        let index = outputs.0.len();
        outputs.0.push(Output::default());
        let output: WlOutput =
            globals
                .registry()
                .bind(global.name, global.version.min(4), &handle, index);
        if let Some(manager) = &manager {
            manager.get_xdg_output(&output, &handle, index);
        }
    }
    queue.roundtrip(&mut outputs).ok()?;
    Some(outputs.0)
}

impl Dispatch<WlRegistry, GlobalListContents> for Outputs {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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

impl Dispatch<WlOutput, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let output = &mut state.0[*index];
        match event {
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                output.mode_long_side = width.max(height);
            }
            wl_output::Event::Name { name } => output.name = name,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let output = &mut state.0[*index];
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.logical_long_side = width.max(height);
            }
            zxdg_output_v1::Event::Name { name } => output.name = name,
            _ => {}
        }
    }
}
