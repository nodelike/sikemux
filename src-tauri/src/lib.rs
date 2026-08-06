mod agent_detection;
mod agents;
mod aws;
mod bruno;
pub mod cli_client;
mod cli_install;
mod cli_protocol;
mod cli_server;
mod diff;
mod error;
mod external;
mod files;
mod fs;
mod fs_watch;
mod git;
mod lsp;
mod pty;
mod rundeck;
mod search;
mod settings;
mod ssh;
mod state;
mod system;
mod transparency;
mod updates;

use aws::LogsTailManager;
use pty::PtyManager;
use rundeck::{RundeckLogsManager, RundeckWatchManager};
use tauri::Manager;

pub fn run() {
    system::normalize_user_environment();

    // Raise our open-file-descriptor limit FIRST, before any subsystem
    // opens an fd. macOS launchd hands GUI apps a soft RLIMIT_NOFILE of 256;
    // a heavy multi-terminal/agent/project session holds far more than that
    // (one fd per PTY + webview + language servers + watchers + sockets) and
    // would otherwise hit EMFILE — git ops, process spawns, and file opens
    // all start failing with "Too many open files".
    system::raise_fd_limit();

    // Inherit the user's shell PATH so spawned subprocesses (hermes for
    // AI commits, rnd CLI, aws CLI, claude, etc.) resolve the same way
    // they do in `make dev`. macOS GUI launches otherwise get a minimal
    // PATH that's missing ~/.local/bin, /opt/homebrew/bin, etc.
    system::fix_path_from_login_shell();

    tauri::Builder::default()
        // Must be the first plugin: subsequent GUI launches focus the primary
        // process instead of creating a second workspace/CLI broker.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(LogsTailManager::default())
        .manage(RundeckWatchManager::default())
        .manage(RundeckLogsManager::default())
        .on_window_event(|window, event| {
            // Drain every live PTY on close so we don't leave orphan
            // shells, agents, or `tail`s alive after the user quits.
            // The OS reaps eventually, but explicit kill avoids the
            // "still using AI tokens" surprise from a backgrounded agent.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                use tauri::Manager;
                if let Some(mgr) = window.try_state::<PtyManager>() {
                    mgr.drain();
                }
                lsp::drain_all();
            }
        })
        .on_page_load(|webview, payload| {
            // Context-menu reload starts a new page without closing the
            // native window, so React cleanup is not a reliable place to
            // kill PTYs. Initial startup has no PTYs yet; reload does.
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                use tauri::Manager;
                if let Some(mgr) = webview.try_state::<PtyManager>() {
                    mgr.drain();
                }
                lsp::drain_all();
            }
        })
        .setup(|_app| {
            let cli_broker = match cli_server::CliBroker::start(_app.handle().clone()) {
                Ok(cli_broker) => Some(cli_broker),
                Err(error) => {
                    eprintln!("Sikemux CLI integration is unavailable: {error}");
                    None
                }
            };
            _app.manage(cli_server::CliBrokerState(cli_broker));
            // See-through window — same recipe as nackle (NSWindow opaque=NO,
            // CGS background blur via private API). No NSVisualEffectView
            // because its frosted look is heavier than the gaussian CGS blur
            // Terminal.app / iTerm2 / Ghostty use. Default blur=0 == pure
            // transparency; the settings slider goes 0..80.
            #[cfg(target_os = "macos")]
            {
                use tauri::Manager;
                if let Some(window) = _app.get_webview_window("main") {
                    if let Ok(handle) = window.ns_window() {
                        unsafe {
                            transparency::apply(handle, 0);
                        }
                    }
                }
            }
            Ok(())
        })
        .manage(PtyManager::default())
        .invoke_handler(tauri::generate_handler![
            pty::pty_spawn,
            pty::pty_subscribe,
            pty::pty_unsubscribe,
            pty::pty_attach,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_reset_modes,
            pty::pty_kill,
            pty::agent_detection_explain,
            pty::agent_detection_manifests,
            pty::agent_detection_reload,
            system::home_dir,
            system::recent_dirs,
            system::boot_init,
            system::battery_status,
            system::runtime_diagnostics,
            system::integration_health,
            updates::update_check,
            updates::update_install,
            state::state_load,
            state::state_save,
            agents::available_agents,
            agents::agent_sessions,
            agents::agent_sessions_watch_start,
            agents::agent_sessions_watch_stop,
            fs::read_dir,
            fs::read_file,
            fs::read_text_file_limited,
            fs::read_file_base64,
            fs::write_file,
            fs::write_file_new,
            fs::create_file,
            fs::create_dir,
            fs::copy_into_dir,
            fs::rename_path,
            fs::reveal_in_finder,
            fs::delete_path,
            fs_watch::repo_watch_start,
            fs_watch::repo_watch_stop,
            git::git_status,
            git::git_diff,
            git::git_stage,
            git::git_unstage,
            git::git_stage_all,
            git::git_unstage_all,
            git::git_branches,
            git::git_checkout,
            git::git_checkout_smart,
            git::git_branch_create,
            git::git_branch_delete,
            git::git_branch_rename,
            git::git_merge,
            git::git_merge_squash,
            git::git_reset,
            git::git_revert,
            git::git_log,
            git::git_overview,
            git::git_show,
            git::git_file_at,
            git::git_commit_files,
            git::git_blame,
            git::git_commit,
            git::git_push,
            git::git_pull,
            git::git_ai_commit,
            git::git_ai_message,
            git::pr_open,
            git::git_discard_file,
            git::git_stash_list,
            git::git_stash_push,
            git::git_stash_apply,
            git::git_stash_pop,
            git::git_stash_drop,
            git::git_stash_branch,
            git::git_stash_rename,
            git::git_remotes,
            git::git_remote_add,
            git::git_remote_remove,
            git::git_remote_rename,
            git::git_remote_set_url,
            git::git_fetch,
            git::git_remote_branches,
            git::git_checkout_remote_branch,
            git::git_delete_remote_branch,
            git::git_set_upstream,
            lsp::lsp_install_server,
            lsp::lsp_start,
            lsp::lsp_stop,
            lsp::lsp_open,
            lsp::lsp_change,
            lsp::lsp_change_incremental,
            lsp::lsp_save,
            lsp::lsp_close,
            lsp::lsp_locations,
            diff::diff_hunks,
            files::list_project_files,
            settings::scan_project_roots,
            settings::expand_path,
            settings::is_directory,
            search::project_search,
            search::project_search_cancel,
            search::project_search_replace,
            search::read_file_window,
            ssh::ssh_hosts,
            ssh::ssh_config_ensure,
            aws::auth::aws_profiles,
            aws::auth::aws_caller_identity,
            aws::auth::aws_sso_login,
            aws::ecs::aws_ecs_clusters,
            aws::ecs::aws_ecs_services,
            aws::ecs::aws_ecs_tasks,
            aws::ecs::aws_ecs_service_log_config,
            aws::ecs::aws_ecs_task_log_config,
            aws::ec2::aws_ec2_instances,
            aws::lambda::aws_lambda_functions,
            aws::sqs::aws_sqs_queues,
            aws::billing::aws_billing_months,
            aws::s3::aws_s3_buckets,
            aws::logs::aws_logs_tail_start,
            aws::logs::aws_logs_tail_stop,
            rundeck::auth::rnd_status,
            rundeck::auth::rnd_login,
            rundeck::auth::rnd_logout,
            rundeck::projects::rnd_projects,
            rundeck::projects::rnd_jobs,
            rundeck::projects::rnd_branches_matrix,
            rundeck::projects::rnd_resolve_job,
            rundeck::executions::rnd_executions,
            rundeck::executions::rnd_execution,
            rundeck::executions::rnd_execution_state,
            rundeck::executions::rnd_run,
            rundeck::executions::rnd_abort,
            rundeck::watch::rnd_watch_start,
            rundeck::watch::rnd_watch_stop,
            rundeck::logs::rnd_logs_start,
            rundeck::logs::rnd_logs_stop,
            rundeck::plan::rnd_plan,
            external::open_url,
            external::macos_focus_app,
            external::run_background_command,
            transparency::set_window_blur,
            bruno::bru_send,
            cli_server::cli_frontend_ready,
            cli_server::cli_claim_open_requests,
            cli_server::cli_open_result,
            cli_server::cli_editor_tabs_closed,
            cli_server::cli_runtime_info,
            cli_install::cli_install_status,
            cli_install::cli_install,
        ])
        .build(tauri::generate_context!())
        .expect("error while building sikemux")
        .run(|app_handle, event| {
            // The window-close and reload hooks above only fire on their
            // specific events. An in-app update relaunches via the process
            // plugin's `relaunch()` → `app.restart()`, which raises
            // RunEvent::ExitRequested then RunEvent::Exit but NO window
            // CloseRequested — so without this hook an update would restart
            // the process while every live shell/agent is abandoned to the
            // kernel's PTY hangup (and anything ignoring SIGHUP would leak).
            // RunEvent::Exit fires on EVERY teardown route — quit, `exit()`,
            // and restart — and runs before the process is actually replaced.
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                if let Some(state) = app_handle.try_state::<cli_server::CliBrokerState>() {
                    if let Some(broker) = &state.0 {
                        broker.shutdown();
                    }
                }
                if let Some(mgr) = app_handle.try_state::<PtyManager>() {
                    mgr.drain();
                }
                lsp::drain_all();
            }
        });
}
