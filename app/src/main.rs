use gpui_kit::{
    point, px, size, AppContext, Bounds, Styled, TitlebarOptions, WindowBounds, WindowDecorations,
    WindowOptions,
};
use sikemux_gpui::{console, core, icons, memory, theme, workbench, WINDOW_SIZE};
use sikemux_host::crash_reports::{self, Process};

fn main() {
    #[cfg(target_os = "linux")]
    hand_cli_to_core();
    let began = std::time::SystemTime::now();
    sikemux_host::panic_reports::record_panics("app");
    crash_reports::start(Process::App, None);
    #[cfg(target_os = "linux")]
    {
        sikemux_browser::choose_page_rendering();
        sikemux_gpui::xwayland_scale::choose();
    }
    console::install();
    let (handed, opened) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(target_os = "linux")]
    if !sikemux_gpui::second_start::take_over(handed.clone()) {
        return;
    }
    let linking = std::time::SystemTime::now();
    let link = core::start(core::Home::default());
    let linked = std::time::SystemTime::now();
    let app = gpui_kit::application();
    #[cfg(target_os = "linux")]
    let app = app.with_windowing(gpui_kit::WindowingRequest::Windowed(x11_session()));
    let app = app.with_assets(icons::Icons);
    app.on_open_urls(move |urls| {
        let _ = handed.send(urls);
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        memory::log(cx);
        cx.set_app_identity(core::IDENTIFIER, core::NAME);
        let behind = std::env::var_os(sikemux_process::vars::LAUNCH_BEHIND).is_some();
        #[cfg(feature = "drive")]
        let behind = behind || sikemux_gpui::drive::stays_behind();
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(860.), px(540.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Sikemux".into()),
                appears_transparent: cfg!(target_os = "macos"),
                // The top of the 14 pt lights, which centres them in the 36 pt top bar.
                traffic_light_position: cfg!(target_os = "macos").then(|| point(px(16.), px(11.))),
            }),
            window_decorations: Some(WindowDecorations::Server),
            app_id: Some(core::IDENTIFIER.into()),
            focus: !behind,
            ..Default::default()
        };
        let (window, bench) = gpui_kit::open_window(options, cx, |window, cx| {
            if let Err(error) = window.enable_scene_overlay() {
                eprintln!("sikemux: pages will cover menus, with no overlay plane: {error}");
            }
            cx.new(|cx| workbench::Workbench::new(link, window, cx))
        })
        .expect("window opens");
        // GPUI Kit's root fills the window with its theme's background, over the ground
        // that the panes' grain and picture paint on. The shell sets that ground itself.
        if let Some(root) = window.downcast::<gpui_kit::base::Root>() {
            let _ = root.update(cx, |root, _, _| {
                root.style().background = Some(gpui_kit::transparent_black().into());
            });
        }
        crash_reports::started(
            began,
            vec![
                ("start the core", linking, linked),
                ("open the window", linked, std::time::SystemTime::now()),
            ],
        );
        cx.on_app_quit(|_| async { crash_reports::stop() }).detach();
        sikemux_gpui::follow_opened(bench.clone(), opened, cx);
        #[cfg(feature = "drive")]
        sikemux_gpui::drive::listen(window, bench, cx);
        if !behind {
            cx.activate(true);
        }
    });
}

/// Browser pages are X11 windows inside the app's, so in a Wayland session the app takes
/// XWayland's display when there is one.
#[cfg(target_os = "linux")]
fn x11_session() -> gpui_kit::GraphicalEnvironment {
    let mut session = gpui_kit::GraphicalEnvironment::detect();
    if session.x11_display.is_some() {
        session.wayland_display = None;
    }
    session
}

/// A Linux package puts the window on PATH as `sikemux`, so `sikemux login` and the other
/// commands reach it; they belong to the core's CLI. The window itself is given only links.
#[cfg(target_os = "linux")]
fn hand_cli_to_core() {
    use std::os::unix::process::CommandExt;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let Some(first) = args.first() else {
        return;
    };
    if first.to_string_lossy().contains("://") {
        return;
    }
    let Some(core) = sikemux_host::cli_paths::cli_executable_path() else {
        eprintln!("sikemux: the core is not installed beside the app");
        std::process::exit(1);
    };
    // The CLI takes this process's place, in the person's own shell, which already has their PATH.
    #[allow(clippy::disallowed_methods)]
    let error = std::process::Command::new(core).args(args).exec();
    eprintln!("sikemux: could not run the core: {error}");
    std::process::exit(1);
}
