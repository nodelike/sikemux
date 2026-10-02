mod acp;
mod activity;
mod agents;
#[cfg(target_os = "macos")]
mod app_menu;
mod autopsy;
mod browser;
pub mod cli_client;
mod cli_install;
mod cli_open;
mod cli_paths;
mod deep_link;
mod diff;
mod document_preview;
mod error;
mod external;
mod file_serving;
mod files;
mod fs;
mod fs_watch;
mod git;
mod grammars;
mod harness;
mod login_item;
mod lsp;
mod markdown;
mod model_providers;
pub mod observability;
mod plugins;
mod ports;
mod preview;
mod pty;
mod release_credits;
mod remote;
mod search;
mod settings;
mod ssh;
mod state;
mod system;
mod transparency;
mod updates;
mod usage;
mod voice;
mod voice_models;
mod wallpaper;
mod wheel;
mod without_page_script;

use acp::AcpManager;
use browser::BrowserManager;
use observability::UiWatchdogState;
use plugins::PluginHost;
use pty::PtyManager;
use sikemux_process as bounded_process;
use tauri::Manager;
use voice::VoiceManager;

/// The build of this app and of the sidecar bundled with it, which runs the
/// background core. Both are compiled with the same build script output.
pub fn build_identity() -> sikemux_core::protocol::BuildIdentity {
    sikemux_core::protocol::BuildIdentity::new(
        env!("CARGO_PKG_VERSION"),
        env!("SIKEMUX_BUILD_COMMIT"),
        env!("SIKEMUX_BUILD_TIME").parse().unwrap_or(0),
        env!("SIKEMUX_BUILD_SOURCE"),
    )
}

// reqwest is built without a TLS crypto backend of its own, so every HTTP
// client in the app and its plugins uses the one installed here.
pub(crate) fn install_tls_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// The app window only ever shows the app. A link that would load another
/// page in it would replace the whole workspace and end every running shell.
fn main_window_may_load(url: &tauri::Url) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
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
    system::avoid_webkit_dmabuf_renderer_on_nvidia();

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
    sikemux_process::user_environment::provide(system::user_environment);
    std::thread::spawn(sikemux_process::user_environment::warm);

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
        .plugin(without_page_script::without_page_script(
            tauri_plugin_dialog::init(),
        ))
        .plugin(without_page_script::without_page_script(
            tauri_plugin_notification::init(),
        ))
        .plugin(main_window_navigation_guard())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
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
                if let Some(plugins) = window.try_state::<PluginHost>() {
                    plugins.drain();
                }
                lsp::drain_all();
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
                if let Some(plugins) = webview.try_state::<PluginHost>() {
                    plugins.drain();
                }
                lsp::drain_all();
            }
        })
        .setup(|_app| {
            _app.manage(UiWatchdogState::start()?);
            model_providers::init(_app.path().app_data_dir()?.join("model-providers.json"));
            _app.manage(PluginHost::with_builtins(
                &_app.path().app_data_dir()?.join("plugins"),
                &_app.package_info().version,
            )?);
            wheel::watch(_app.handle());
            _app.state::<PtyManager>().start(_app.handle());
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
            }
            Ok(())
        })
        .manage(deep_link::DeepLinks::default())
        .manage(PtyManager::default())
        .manage(harness::HarnessBroker::default())
        .manage(cli_open::CliOpens::default())
        .manage(AcpManager::default())
        .manage(remote::PublishedWorkspace::default())
        .manage(remote::PublishedChats::default())
        .manage(remote::PublishedPalette::default())
        .manage(remote::PublishedBackdrop::default())
        .manage(BrowserManager::default())
        .manage(VoiceManager::default())
        .manage(preview::Previews::default())
        .register_asynchronous_uri_scheme_protocol(preview::SCHEME, preview::handle)
        .invoke_handler(tauri::generate_handler![
            acp::acp_start,
            acp::acp_attach,
            acp::acp_list,
            acp::acp_prompt,
            acp::acp_set_permission_mode,
            acp::acp_set_config,
            acp::acp_cancel,
            acp::acp_steer,
            acp::acp_stop_task,
            acp::acp_permission_reply,
            acp::acp_stop,
            pty::commands::pty_spawn,
            pty::commands::task_spawn,
            pty::commands::pty_subscribe,
            pty::commands::pty_unsubscribe,
            pty::commands::pty_ack,
            pty::commands::pty_attach,
            pty::commands::pty_write,
            pty::commands::pty_resize,
            pty::commands::pty_reset_modes,
            pty::commands::pty_kill,
            ports::listening_ports,
            pty::commands::pty_sessions,
            remote::remote_status,
            remote::remote_set_enabled,
            remote::remote_set_device_access,
            remote::remote_revoke_device,
            remote::remote_open_pairing,
            remote::remote_close_pairing,
            remote::remote_answer_pairing,
            remote::remote_publish_workspace,
            remote::remote_publish_chats,
            remote::remote_publish_palette,
            remote::remote_publish_backdrop,
            pty::commands::task_watch,
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
            system::home_dir,
            system::recent_dirs,
            system::boot_init,
            system::battery_status,
            system::runtime_diagnostics,
            system::integration_health,
            observability::observability_ui_heartbeat,
            observability::observability_ui_activity,
            autopsy::hang_reports,
            updates::update_check,
            updates::update_install,
            usage::usage_report_active,
            release_credits::release_avatars,
            release_credits::release_notes,
            grammars::grammar_load,
            state::state_load,
            state::state_save,
            agents::executable::available_agents,
            agents::models::agent_models,
            agents::usage::agent_usage,
            agents::sessions::agent_sessions,
            agents::sessions::recent::agent_recent_sessions,
            agents::sessions::context::agent_session_context,
            agents::sessions::rename::agent_session_rename,
            agents::sessions::live_agent_sessions,
            agents::watch::agent_sessions_watch_start,
            agents::watch::agent_sessions_watch_stop,
            activity::activity_turn_started,
            activity::activity_turn_ended,
            activity::activity_summary,
            fs::read_dir,
            fs::read_dirs,
            fs::path_kinds,
            fs::read_file,
            fs::read_file_versioned,
            fs::read_text_file_limited,
            fs::open_in_default_app,
            preview::preview_file,
            document_preview::document_preview_show,
            document_preview::document_preview_hide,
            fs::write_file,
            fs::write_file_versioned,
            fs::write_file_new,
            fs::create_file,
            fs::create_dir,
            fs::copy_into_dir,
            fs::downloads_dir,
            fs::chat_attachment_dir,
            fs::save_clipboard_image,
            fs::save_base64_into_dir,
            fs::rename_path,
            fs::reveal_in_finder,
            fs::delete_path,
            fs_watch::repo_watch_start,
            fs_watch::repo_watch_stop,
            git::status::git_status,
            git::status::git_discover_repos,
            git::changes::git_diff,
            git::changes::git_stage,
            git::changes::git_unstage,
            git::changes::git_stage_paths,
            git::changes::git_unstage_paths,
            git::changes::git_stage_all,
            git::changes::git_unstage_all,
            git::branches::git_branches,
            git::worktree::git_worktree_list,
            git::worktree::git_worktree_create,
            git::worktree::git_worktree_remove,
            git::branches::git_checkout,
            git::branches::git_checkout_smart,
            git::branches::git_branch_create,
            git::branches::git_branch_delete,
            git::branches::git_branch_rename,
            git::branches::git_merge,
            git::branches::git_merge_squash,
            git::branches::git_reset,
            git::branches::git_revert,
            git::log::git_log,
            git::log::git_overview,
            git::revisions::git_show,
            git::revisions::git_file_at,
            git::revisions::git_file_diff,
            git::revisions::git_commit_files,
            git::revisions::git_compare,
            git::blame::git_blame,
            git::commit::git_commit,
            git::commit::git_push,
            git::commit::git_pull,
            git::ai::git_ai_commit,
            git::ai::git_ai_message,
            git::remote::pr_open,
            git::changes::git_discard_file,
            git::changes::git_discard_files,
            git::stash::git_stash_list,
            git::stash::git_stash_push,
            git::stash::git_stash_apply,
            git::stash::git_stash_pop,
            git::stash::git_stash_drop,
            git::stash::git_stash_branch,
            git::stash::git_stash_rename,
            git::remote::git_remotes,
            git::remote::git_remote_add,
            git::remote::git_remote_remove,
            git::remote::git_remote_rename,
            git::remote::git_remote_set_url,
            git::remote::git_fetch,
            git::remote::git_fetch_ref,
            git::remote::git_remote_branches,
            git::remote::git_checkout_remote_branch,
            git::remote::git_delete_remote_branch,
            git::remote::git_set_upstream,
            lsp::lsp_install_server,
            lsp::lsp_start,
            lsp::lsp_stop,
            lsp::lsp_open,
            lsp::lsp_change,
            lsp::lsp_change_incremental,
            lsp::lsp_save,
            lsp::lsp_close,
            lsp::lsp_locations,
            lsp::lsp_document_symbols,
            diff::diff_hunks,
            markdown::markdown_parse,
            files::list_project_files,
            files::list_project_files_snapshot,
            settings::scan_project_roots,
            model_providers::model_providers,
            model_providers::model_provider_connect,
            model_providers::model_provider_disconnect,
            settings::expand_path,
            settings::is_directory,
            wallpaper::wallpaper_image,
            search::project_search,
            search::project_search_cancel,
            search::project_search_replace,
            search::read_file_window,
            ssh::ssh_hosts,
            ssh::ssh_config_ensure,
            external::open_url,
            external::macos_focus_app,
            external::run_background_command,
            transparency::set_window_blur,
            plugins::plugin_manifests,
            plugins::plugin_set_disabled,
            plugins::plugin_call,
            plugins::plugin_stream_start,
            plugins::plugin_stream_stop,
            harness::harness_resolve_path,
            harness::harness_claim,
            harness::harness_reply,
            harness::harness_awaiting_trust,
            harness::harness_stop_runs,
            cli_open::cli_frontend_ready,
            cli_open::cli_claim_open_requests,
            cli_open::cli_open_result,
            cli_open::cli_editor_tabs_closed,
            cli_paths::cli_runtime_info,
            deep_link::take_deep_links,
            cli_install::cli_install_status,
            cli_install::cli_install,
            voice::voice_status,
            voice::voice_prepare,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_shutdown,
        ])
        .build(tauri::generate_context!())
        .expect("error while building sikemux")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                deep_link::receive(app_handle, urls);
            }
            // RunEvent::Exit fires on every teardown route: quit, `exit()`,
            // and the restart after an in-app update, which raises no window
            // CloseRequested. Terminals and chat agents stay in the core on
            // all of them; only "Quit and Stop Everything" stops them, before
            // it exits.
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
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
                if let Some(plugins) = app_handle.try_state::<PluginHost>() {
                    plugins.drain();
                }
                if let Some(voice) = app_handle.try_state::<VoiceManager>() {
                    voice.drain();
                }
                lsp::drain_all();
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
                crate::files::list_project_files_snapshot,
                crate::observability::observability_ui_heartbeat,
            ])
            .build(mock_context(noop_assets()))
            .expect("build MockRuntime application");
        let webview = WebviewWindowBuilder::new(&app, "command-boundary", Default::default())
            .build()
            .expect("build mock webview");

        let snapshot = get_ipc_response(
            &webview,
            request(
                "list_project_files_snapshot",
                json!({ "repo": repo.path().to_string_lossy() }),
            ),
        )
        .expect("filesystem command must cross the IPC boundary successfully")
        .deserialize::<Value>()
        .expect("filesystem response must be JSON");

        let scan_id = snapshot
            .get("scanId")
            .and_then(Value::as_u64)
            .expect("response must expose the frontend camelCase scanId field");
        assert!(scan_id > 0);
        assert!(snapshot.get("scan_id").is_none());
        let mut files = snapshot["files"]
            .as_array()
            .expect("response files must be an array")
            .iter()
            .map(|path| {
                path.as_str()
                    .expect("response paths must be strings")
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        files.sort_unstable();
        assert_eq!(files, ["README.md", "src/main.rs"]);

        let error = get_ipc_response(
            &webview,
            request(
                "list_project_files_snapshot",
                json!({ "repo": "relative-path-is-rejected" }),
            ),
        )
        .expect_err("command errors must cross the IPC error callback");
        assert_eq!(
            error,
            json!("repository path must be an absolute path of at most 4096 bytes")
        );

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
