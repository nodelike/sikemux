mod account;
mod acp;
mod agent_notification;
mod agent_questions;
#[cfg(target_os = "macos")]
mod app_menu;
mod autopsy;
mod browser;
mod cli_open;
mod core_call;
mod deep_link;
mod dialogs;
mod diff;
mod document_preview;
mod error;
mod external;
mod frame_rate;
mod fs;
mod grammars;
mod harness;
mod hosts;
mod inbox;
mod markdown;
pub mod observability;
mod pages;
mod plugins;
mod preview;
mod pty;
mod relaunch;
mod remote;
mod share;
mod simulator;
mod state;
mod system;
mod transparency;
mod voice;
mod wallpaper;
mod wheel;

use sikemux_host::{build_identity, cli_paths, install_tls_crypto, login_item};

use acp::AcpManager;
use browser::BrowserManager;
use observability::UiWatchdogState;
use pty::PtyManager;
use sikemux_process as bounded_process;
use simulator::SimulatorManager;
use tauri::Manager;
use voice::VoiceManager;

/// The app window only ever shows the app. A link that would load another
/// page in it would replace the whole workspace and end every running shell.
/// WebKit asks the same of every frame, so the chat's page frames pass too.
fn main_window_may_load(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" | "page" => url.host_str() == Some("localhost"),
        "http" if cfg!(debug_assertions) => {
            url.host_str() == Some("localhost") && url.port() == Some(1420)
        }
        _ => false,
    }
}

fn main_window_navigation_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("main-window-navigation")
        .on_navigation(|webview, url| webview.label() != "main" || main_window_may_load(url))
        .build()
}

static MAIN_PAGE_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn run() {
    install_tls_crypto();
    system::normalize_user_environment();
    #[cfg(target_os = "linux")]
    system::keep_webkit_rendering_on_nvidia();

    // Raise our open-file-descriptor limit FIRST, before any subsystem
    // opens an fd. macOS launchd hands GUI apps a soft RLIMIT_NOFILE of 256;
    // a heavy multi-terminal/agent/project session holds far more than that
    // (one fd per PTY + webview + language servers + watchers + sockets) and
    // would otherwise hit EMFILE — git ops, process spawns, and file opens
    // all start failing with "Too many open files".
    system::raise_fd_limit();

    // Children get the user's shell PATH so hermes, rnd, aws, claude, etc.
    // resolve the way they do in `make dev`. Reading the login shell takes as
    // long as the user's rc files, so it runs while the window is created.
    cli_paths::link_cli_for_children();
    sikemux_process::user_environment::provide(
        system::user_environment,
        sikemux_pty::user_shell::login_shell_generation,
    );
    std::thread::spawn(sikemux_process::user_environment::warm);

    frame_rate::render_at_display_rate();

    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu::build).on_menu_event(|app, event| {
        if event.id() == app_menu::QUIT_AND_STOP_EVERYTHING {
            pty::quit_and_stop_everything(app);
        }
    });
    builder
        // Must be the first plugin: subsequent GUI launches focus the primary
        // process instead of creating a second workspace/CLI broker.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(window) = app.get_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(main_window_navigation_guard())
        .plugin(tauri_plugin_process::init())
        .on_window_event(|window, event| {
            // The core sends agents' window calls, such as their browser's, to
            // whichever Sikemux window the person brought forward last.
            if let tauri::WindowEvent::Focused(true) = event {
                use tauri::Manager;
                if let Some(client) = window
                    .try_state::<PtyManager>()
                    .and_then(|manager| manager.current_client())
                {
                    tauri::async_runtime::spawn(async move {
                        let _ = client.register_window().await;
                    });
                }
            }
            // Terminals, terminal agents, tasks and chat agents live in the
            // core and keep running after the window closes; the next launch
            // reattaches them.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                use tauri::Manager;
                if let Some(watchdog) = window.try_state::<UiWatchdogState>() {
                    watchdog.suspend();
                }
                if let Some(mgr) = window.try_state::<PtyManager>() {
                    mgr.detach_all();
                }
                if let Some(harness) = window.try_state::<harness::HarnessBroker>() {
                    harness.fail_all("Sikemux's window closed before it answered");
                }
                if let Some(browser) = window.try_state::<BrowserManager>() {
                    browser.drain("the window closed");
                }
            }
        })
        .on_page_load(|webview, payload| {
            // Context-menu reload starts a new page without closing the
            // native window, so React cleanup never runs. Terminals and chat
            // agents keep running in the core and the new page reattaches them.
            // Browser tabs are webviews too, and a page loading in one of
            // them is not the app reloading. Nor is the window's first page:
            // an agent may already have opened a tab before it commits.
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Started
                && MAIN_PAGE_LOADED.swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                use tauri::Manager;
                autopsy::forget_web_content_pid();
                document_preview::clear(webview.app_handle());
                if let Some(watchdog) = webview.try_state::<UiWatchdogState>() {
                    watchdog.suspend();
                }
                if let Some(mgr) = webview.try_state::<PtyManager>() {
                    mgr.detach_all();
                    mgr.update_core_if_stale();
                }
                if let Some(harness) = webview.try_state::<harness::HarnessBroker>() {
                    harness.fail_all("Sikemux's window reloaded before it answered");
                }
                if let Some(browser) = webview.try_state::<BrowserManager>() {
                    browser.drain(&format!("the window started loading {}", payload.url()));
                }
                if let Some(simulators) = webview.try_state::<SimulatorManager>() {
                    simulators.sim().drop_watches();
                }
            }
        })
        .setup(|_app| {
            sikemux_core::runtime::set(tauri::async_runtime::handle().inner().clone());
            _app.manage(browser::engine(_app.handle()));
            _app.manage(UiWatchdogState::start()?);
            _app.manage(simulator::engine(_app.handle()));
            relaunch::restore(_app.handle());
            tauri::async_runtime::spawn_blocking(sikemux_simulator::offered);
            wheel::watch(_app.handle());
            _app.state::<PtyManager>().start(_app.handle());
            _app.state::<hosts::Hosts>().start(_app.handle());
            // See-through window — same recipe as nackle (NSWindow opaque=NO,
            // CGS background blur via private API). No NSVisualEffectView
            // because its frosted look is heavier than the gaussian CGS blur
            // Terminal.app / iTerm2 / Ghostty use. Default blur=0 == pure
            // transparency; the settings slider goes 0..80.
            #[cfg(target_os = "macos")]
            {
                use tauri::Manager;
                if let Some(window) = _app.get_window("main") {
                    if let Ok(handle) = window.ns_window() {
                        // SAFETY: `ns_window()` is the main window's live NSWindow, and
                        // Tauri runs setup on the main thread.
                        unsafe {
                            transparency::apply(handle, 0);
                        }
                    }
                }
                // The desktop smoke test runs behind other windows, where WebKit would pause this page.
                if std::env::var_os("SIKEMUX_RUN_WHEN_COVERED").is_some() {
                    if let Some(webview) = _app.get_webview("main") {
                        let _ = webview.with_webview(|platform| {
                            // SAFETY: Tauri runs this on the main thread with the main
                            // window's live WKWebView.
                            let main = unsafe {
                                platform.inner().cast::<objc2_web_kit::WKWebView>().as_ref()
                            };
                            if let Some(main) = main {
                                browser::macos::keep_shown_page_running_when_covered(main);
                            }
                        });
                    }
                }
            }
            Ok(())
        })
        .manage(deep_link::DeepLinks::default())
        .manage(PtyManager::default())
        .manage(hosts::Hosts::default())
        .manage(harness::HarnessBroker::default())
        .manage(cli_open::CliOpens::default())
        .manage(AcpManager::default())
        .manage(remote::Published::default())
        .manage(agent_notification::WaitingFocus::default())
        .manage(VoiceManager::default())
        .manage(preview::Previews::default())
        .register_asynchronous_uri_scheme_protocol(preview::SCHEME, preview::handle)
        .manage(pages::Pages::default())
        .register_asynchronous_uri_scheme_protocol(pages::SCHEME, pages::handle)
        .invoke_handler(tauri::generate_handler![
            acp::acp_attach,
            acp::acp_stop,
            acp::acp_switch_account,
            pty::commands::pty_spawn,
            pty::commands::pty_unsubscribe,
            pty::commands::pty_ack,
            pty::commands::pty_attach,
            pty::commands::pty_input,
            pty::commands::pty_lines,
            pty::commands::pty_text,
            pty::commands::pty_find,
            pty::commands::pty_clear,
            pty::commands::pty_write,
            pty::commands::pty_call,
            pty::commands::pty_resize,
            pty::commands::pty_reset_modes,
            pty::commands::pty_kill,
            pty::commands::pty_sessions,
            hosts::install::host_install,
            hosts::install::host_sign_in,
            hosts::install::host_update,
            hosts::install::host_start,
            share::share_access,
            share::share_viewer_hello,
            core_call::core_call,
            core_call::core_request,
            remote::remote_publish,
            pty::commands::app_quit_and_stop_everything,
            pty::commands::agent_detection_explain,
            pty::commands::agent_detection_manifests,
            pty::commands::agent_detection_reload,
            browser::browser_snapshot,
            browser::browser_new_tab,
            browser::browser_close_agent,
            browser::browser_switch_tab,
            browser::browser_close_tab,
            browser::browser_navigate,
            browser::browser_suggest,
            browser::browser_back,
            browser::browser_forward,
            browser::browser_reload,
            browser::browser_set_bounds,
            browser::browser_page_still,
            system::boot_init,
            system::battery_status,
            system::runtime_diagnostics,
            observability::observability_ui_heartbeat,
            observability::observability_ui_activity,
            pty::commands::update_force_core,
            relaunch::update_restart,
            grammars::grammar_load,
            state::state_save,
            fs::keep_dropped_paths,
            fs::open_in_default_app,
            preview::preview_file,
            pages::page_theme,
            pages::page_shown,
            document_preview::document_preview_show,
            document_preview::document_preview_hide,
            fs::downloads_dir,
            fs::chat_attachment_dir,
            fs::save_clipboard_image,
            fs::reveal_in_finder,
            diff::diff_hunks,
            markdown::markdown_parse,
            wallpaper::wallpaper_image,
            agent_notification::post_notification,
            agent_notification::focus_claim,
            dialogs::pick_paths,
            dialogs::pick_save_path,
            external::open_url,
            external::macos_focus_app,
            transparency::set_window_blur,
            plugins::plugin_call,
            harness::harness_resolve_path,
            harness::harness_claim,
            harness::harness_reply,
            cli_open::cli_frontend_ready,
            cli_open::cli_claim_open_requests,
            cli_open::cli_open_result,
            cli_open::cli_editor_tabs_closed,
            deep_link::take_deep_links,
            voice::voice_status,
            voice::voice_prepare,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_shutdown,
            simulator::sim_status,
            simulator::sim_prepare,
            simulator::sim_call,
            simulator::sim_watch,
            simulator::sim_unwatch,
            simulator::sim_watch_read,
            simulator::simulator_set_enabled,
            simulator::simulator_setup,
            simulator::simulator_attachments,
            simulator::simulator_set_desk_device,
            simulator::simulator_orientation,
        ])
        .build(tauri::generate_context!())
        .expect("error while building sikemux")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                deep_link::receive(app_handle, urls);
                let paths = urls.iter().filter_map(|url| url.to_file_path().ok());
                cli_open::open_paths(app_handle, paths.collect());
            }
            if let tauri::RunEvent::ExitRequested { code, .. } = &event {
                relaunch::note_exit_requested(*code);
            }
            // RunEvent::Exit fires on every teardown route: quit, `exit()`,
            // and the restart after an in-app update, which raises no window
            // CloseRequested. Terminals and chat agents stay in the core on
            // all of them; only "Quit and Stop Everything" stops them, before
            // it exits.
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let updating = relaunch::for_update() && relaunch::save(app_handle).is_ok();
                if let Some(watchdog) = app_handle.try_state::<UiWatchdogState>() {
                    watchdog.suspend();
                }
                if let Some(harness) = app_handle.try_state::<harness::HarnessBroker>() {
                    harness.fail_all("Sikemux quit before it answered");
                }
                if let Some(opens) = app_handle.try_state::<cli_open::CliOpens>() {
                    opens.shutdown();
                }
                if let Some(mgr) = app_handle.try_state::<PtyManager>() {
                    mgr.release();
                }
                if let Some(browser) = app_handle.try_state::<BrowserManager>() {
                    browser.drain("Sikemux is quitting");
                }
                if let Some(voice) = app_handle.try_state::<VoiceManager>() {
                    voice.drain();
                }
                if let Some(simulators) = app_handle.try_state::<SimulatorManager>() {
                    if updating {
                        simulators.step_aside();
                    } else {
                        simulators.drain();
                    }
                    simulators.sim().drain();
                }
            }
        });
}

#[cfg(test)]
mod main_window_navigation_tests {
    use super::main_window_may_load;

    #[test]
    fn keeps_the_app_window_on_the_app() {
        let allows = |url: &str| main_window_may_load(&url.parse().unwrap());
        assert!(allows("tauri://localhost/"));
        assert!(allows("tauri://localhost/index.html#settings"));
        assert!(allows("http://localhost:1420/"));
        assert!(allows("page://localhost/0123456789abcdef0123456789abcdef"));
        assert!(!allows("page://evil.example/"));
        assert!(!allows("https://example.com/"));
        assert!(!allows("http://localhost:3000/"));
        assert!(!allows("tauri://evil.example/"));
        assert!(!allows("file:///etc/passwd"));
    }
}

#[cfg(all(test, feature = "ipc-command-tests"))]
mod ipc_command_boundary_tests {
    //! Command-boundary smoke tests for native Tauri IPC.
    //!
    //! These tests dispatch real [`tauri::webview::InvokeRequest`] values
    //! through macro-generated command handlers on Tauri's
    //! [`tauri::test::MockRuntime`]. They deliberately cover native
    //! serialization, dispatch, async work, errors, and managed-state
    //! injection; they are not packaged-WebView E2E tests.

    use serde_json::{json, Value};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::WebviewWindowBuilder;

    use crate::observability::UiWatchdogState;

    fn request(command: &str, body: Value) -> InvokeRequest {
        InvokeRequest {
            cmd: command.to_owned(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test invoke URL must be valid"),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        }
    }

    /// Smoke-tests the real native command boundary, not a packaged WebView.
    #[test]
    fn command_boundary_smoke_dispatches_async_filesystem_work_and_managed_state() {
        let repo = tempfile::tempdir().expect("temporary project root");
        std::fs::create_dir(repo.path().join("src")).expect("create source directory");
        std::fs::write(repo.path().join("README.md"), "# smoke\n").expect("write project file");
        std::fs::write(repo.path().join("src").join("main.rs"), "fn main() {}\n")
            .expect("write nested project file");

        let app = mock_builder()
            .manage(UiWatchdogState::start().expect("start managed watchdog state"))
            .invoke_handler(tauri::generate_handler![
                crate::fs::keep_dropped_paths,
                crate::fs::reveal_in_finder,
                crate::observability::observability_ui_heartbeat,
            ])
            .build(mock_context(noop_assets()))
            .expect("build MockRuntime application");
        let webview = WebviewWindowBuilder::new(&app, "command-boundary", Default::default())
            .build()
            .expect("build mock webview");

        let path = repo.path().join("src").join("main.rs");
        let kept = get_ipc_response(
            &webview,
            request(
                "keep_dropped_paths",
                json!({ "paths": [path.to_string_lossy()] }),
            ),
        )
        .expect("filesystem command must cross the IPC boundary successfully")
        .deserialize::<Value>()
        .expect("filesystem response must be JSON");
        assert_eq!(kept, json!([path.to_string_lossy()]));

        let error = get_ipc_response(
            &webview,
            request(
                "reveal_in_finder",
                json!({ "path": repo.path().join("missing.txt").to_string_lossy() }),
            ),
        )
        .expect_err("command errors must cross the IPC error callback");
        assert!(!error.is_null());

        let heartbeat = get_ipc_response(
            &webview,
            request(
                "observability_ui_heartbeat",
                json!({ "visible": true, "heartbeat": 1 }),
            ),
        )
        .expect("managed-state command must resolve its injected state")
        .deserialize::<Value>()
        .expect("unit response must serialize as JSON");
        assert_eq!(heartbeat, Value::Null);
    }
}
