use gpui_kit::{
    point, px, size, AppContext, Bounds, Styled, TitlebarOptions, WindowBounds, WindowOptions,
};
use sikemux_gpui::{console, core, icons, theme, workbench, WINDOW_SIZE};

fn main() {
    console::install();
    // Until this app replaces the Mac app, a release keeps a core and files of its own, so it
    // never starts or upgrades the core the Mac app uses. A dev build is Sikemux Dev and shares
    // the Mac dev build's.
    if !cfg!(debug_assertions) && std::env::var_os(sikemux_process::vars::CONFIG_DIR).is_none() {
        if let Some(home) = sikemux_process::paths::home() {
            std::env::set_var(
                sikemux_process::vars::CONFIG_DIR,
                home.join(".config").join("sikemux-gpui"),
            );
        }
    }
    let link = core::start(core::Home::default());
    gpui_kit::application()
        .with_assets(icons::Icons)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::install(cx);
            let behind = std::env::var_os(sikemux_process::vars::LAUNCH_BEHIND).is_some();
            #[cfg(feature = "drive")]
            let behind = behind || sikemux_gpui::drive::stays_behind();
            let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(860.), px(540.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Sikemux".into()),
                    appears_transparent: true,
                    // The top of the 14 pt lights, which centres them in the 36 pt top bar.
                    traffic_light_position: Some(point(px(16.), px(11.))),
                }),
                focus: !behind,
                ..Default::default()
            };
            let (window, _bench) = gpui_kit::open_window(options, cx, |window, cx| {
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
            #[cfg(feature = "drive")]
            sikemux_gpui::drive::listen(window, _bench, cx);
            if !behind {
                cx.activate(true);
            }
        });
}
