use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::task;

use super::limits::{server_limit_error, MAX_LSP_SERVERS, OPEN_DOCUMENT_COUNT};
use super::lsp;
use super::server::{shutdown_server, ServerHandle, ServerKey};
use crate::error::AppResult;
use crate::observability::global_observability;

pub(super) fn registry() -> &'static Mutex<HashMap<ServerKey, ServerHandle>> {
    static R: OnceLock<Mutex<HashMap<ServerKey, ServerHandle>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn start_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

pub(super) fn server_for(project: &str, language: &str) -> Option<ServerHandle> {
    registry()
        .lock()
        .ok()?
        .get(&ServerKey::new(project, language))
        .filter(|server| !server.shutdown.load(Ordering::Acquire))
        .cloned()
}

pub fn server_count() -> usize {
    registry()
        .lock()
        .map(|registry| {
            registry
                .values()
                .filter(|server| !server.shutdown.load(Ordering::Acquire))
                .count()
        })
        .unwrap_or(0)
}

pub fn document_counts() -> (usize, usize) {
    let idle_servers = registry()
        .lock()
        .map(|registry| {
            let mut idle_servers = 0usize;
            for server in registry.values() {
                if server.shutdown.load(Ordering::Acquire) {
                    continue;
                }
                match server.open_docs.lock() {
                    Ok(docs) if docs.is_empty() => idle_servers += 1,
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
            idle_servers
        })
        .unwrap_or_default();
    (OPEN_DOCUMENT_COUNT.load(Ordering::Acquire), idle_servers)
}

const LSP_IDLE_GRACE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
struct AdmissionEntry {
    key: ServerKey,
    live: bool,
    idle: bool,
    last_used: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ServerStartAction {
    Existing,
    Admit { victims: Vec<ServerKey> },
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ServerAdmissionPlan {
    stale: Vec<ServerKey>,
    action: ServerStartAction,
}

/// Plan admission without mutating the registry so the all-busy rejection is
/// atomic: we never kill a partial set of servers and then discover that too
/// few idle victims existed. Ties use the typed key for deterministic tests
/// and repeatable eviction behavior.
fn plan_server_admission(
    requested: &ServerKey,
    entries: impl IntoIterator<Item = AdmissionEntry>,
) -> ServerAdmissionPlan {
    let entries = entries.into_iter().collect::<Vec<_>>();
    let mut stale = entries
        .iter()
        .filter(|entry| !entry.live)
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    stale.sort();

    if entries
        .iter()
        .any(|entry| entry.live && entry.key == *requested)
    {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Existing,
        };
    }

    let live_count = entries.iter().filter(|entry| entry.live).count();
    if live_count < MAX_LSP_SERVERS {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Admit {
                victims: Vec::new(),
            },
        };
    }

    let victims_needed = live_count - MAX_LSP_SERVERS + 1;
    let mut idle = entries
        .iter()
        .filter(|entry| entry.live && entry.idle && entry.key != *requested)
        .map(|entry| (entry.last_used, entry.key.clone()))
        .collect::<Vec<_>>();
    idle.sort();
    if idle.len() < victims_needed {
        return ServerAdmissionPlan {
            stale,
            action: ServerStartAction::Reject,
        };
    }

    ServerAdmissionPlan {
        stale,
        action: ServerStartAction::Admit {
            victims: idle
                .into_iter()
                .take(victims_needed)
                .map(|(_, key)| key)
                .collect(),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PreparedServerStart {
    Existing,
    Admit,
}

/// Remove stale entries and any complete idle-victim set while the registry
/// is locked. `start_lock` serializes callers across this preparation, process
/// spawn, and insertion, so registry cardinality can never transiently exceed
/// `MAX_LSP_SERVERS`.
pub(super) fn prepare_server_start(requested: &ServerKey) -> AppResult<PreparedServerStart> {
    let (action, to_shutdown) = {
        let mut registry = registry().lock().map_err(lsp)?;
        let entries = registry
            .iter()
            .map(|(key, server)| AdmissionEntry {
                key: key.clone(),
                live: !server.shutdown.load(Ordering::Acquire),
                idle: server
                    .open_docs
                    .lock()
                    .map(|documents| documents.is_empty())
                    .unwrap_or(false),
                last_used: server.last_used.load(Ordering::Relaxed),
            })
            .collect::<Vec<_>>();
        let plan = plan_server_admission(requested, entries);
        let mut to_shutdown = Vec::new();
        for key in &plan.stale {
            if let Some(server) = registry.remove(key) {
                to_shutdown.push(server);
            }
        }
        if let ServerStartAction::Admit { victims } = &plan.action {
            for key in victims {
                if let Some(server) = registry.remove(key) {
                    to_shutdown.push(server);
                }
            }
        }
        if matches!(plan.action, ServerStartAction::Existing) {
            if let Some(server) = registry.get(requested) {
                // A fresh start cancels any idle-shutdown timer that was
                // scheduled before the frontend decided to reuse the server.
                server.idle_generation.fetch_add(1, Ordering::AcqRel);
            }
        }
        (plan.action, to_shutdown)
    };

    for server in to_shutdown {
        shutdown_server(server);
    }

    match action {
        ServerStartAction::Existing => Ok(PreparedServerStart::Existing),
        ServerStartAction::Admit { .. } => Ok(PreparedServerStart::Admit),
        ServerStartAction::Reject => {
            let _ = global_observability().increment_counter("lsp.server_limit_rejections", 1);
            Err(server_limit_error())
        }
    }
}

pub(super) fn schedule_idle_shutdown(server_key: ServerKey, server: ServerHandle) {
    let generation = server
        .idle_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
        .saturating_add(1);
    task::spawn(async move {
        tokio::time::sleep(LSP_IDLE_GRACE).await;
        if server.shutdown.load(std::sync::atomic::Ordering::Acquire)
            || server
                .idle_generation
                .load(std::sync::atomic::Ordering::Acquire)
                != generation
            || server
                .open_docs
                .lock()
                .map(|docs| !docs.is_empty())
                .unwrap_or(true)
        {
            return;
        }
        let _ = task::spawn_blocking(move || {
            let _start_guard = start_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if server.shutdown.load(Ordering::Acquire)
                || server.idle_generation.load(Ordering::Acquire) != generation
                || server
                    .open_docs
                    .lock()
                    .map(|docs| !docs.is_empty())
                    .unwrap_or(true)
            {
                return;
            }
            let victim = registry().lock().ok().and_then(|mut reg| {
                let current = reg.get(&server_key)?;
                if !Arc::ptr_eq(current, &server) {
                    return None;
                }
                reg.remove(&server_key)
            });
            if let Some(victim) = victim {
                shutdown_server(victim);
            }
        })
        .await;
    });
}

/// Return true only for a usable registry entry. Reader-owned teardown marks
/// failed/exited servers shut down; remove those entries so a subsequent start
/// can actually spawn a replacement.
pub(super) fn live_server_exists(key: &ServerKey) -> AppResult<bool> {
    let stale = {
        let mut registry = registry().lock().map_err(lsp)?;
        match registry.get(key) {
            Some(server) if !server.shutdown.load(std::sync::atomic::Ordering::Relaxed) => {
                return Ok(true)
            }
            Some(_) => registry.remove(key),
            None => None,
        }
    };
    if let Some(server) = stale {
        shutdown_server(server);
    }
    Ok(false)
}

/// Application-teardown backstop for servers whose project stop never arrived.
pub fn drain_all() {
    let _start_guard = start_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let servers = registry()
        .lock()
        .map(|mut registry| {
            registry
                .drain()
                .map(|(_, server)| server)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for server in servers {
        shutdown_server(server);
    }
}

pub(super) fn server_keys_for_project<T>(
    registry: &HashMap<ServerKey, T>,
    project: &str,
) -> Vec<ServerKey> {
    registry
        .keys()
        .filter(|key| key.project == project)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission_entry(
        project: &str,
        language: &str,
        live: bool,
        idle: bool,
        last_used: u64,
    ) -> AdmissionEntry {
        AdmissionEntry {
            key: ServerKey::new(project, language),
            live,
            idle,
            last_used,
        }
    }

    #[test]
    fn seventh_busy_server_is_rejected_before_admission() {
        let requested = ServerKey::new("/project/seven", "rust");
        let entries = (0..MAX_LSP_SERVERS)
            .map(|index| {
                admission_entry(
                    &format!("/project/{index}"),
                    "rust",
                    true,
                    false,
                    index as u64,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            plan_server_admission(&requested, entries),
            ServerAdmissionPlan {
                stale: Vec::new(),
                action: ServerStartAction::Reject,
            }
        );
        assert_eq!(
            server_limit_error().to_string(),
            format!(
                "lsp: language server limit reached ({MAX_LSP_SERVERS}); close a project before starting another"
            )
        );
    }

    #[test]
    fn admission_cleans_stale_entries_and_evicts_complete_lru_set() {
        let requested = ServerKey::new("/project/new", "rust");
        let stale = ServerKey::new("/project/stale", "go");
        let oldest_idle = ServerKey::new("/project/idle-a", "rust");
        let next_idle = ServerKey::new("/project/idle-b", "rust");
        let mut entries = vec![
            admission_entry(&stale.project, &stale.language, false, true, 0),
            admission_entry(&oldest_idle.project, &oldest_idle.language, true, true, 1),
            admission_entry(&next_idle.project, &next_idle.language, true, true, 2),
        ];
        entries.extend((0..MAX_LSP_SERVERS - 1).map(|index| {
            admission_entry(
                &format!("/project/busy-{index}"),
                "rust",
                true,
                false,
                10 + index as u64,
            )
        }));

        // Seven live servers require two complete idle evictions before the
        // requested server can be admitted under a hard cap of six.
        assert_eq!(
            plan_server_admission(&requested, entries),
            ServerAdmissionPlan {
                stale: vec![stale],
                action: ServerStartAction::Admit {
                    victims: vec![oldest_idle, next_idle],
                },
            }
        );
    }

    #[test]
    fn admission_never_selects_a_partial_eviction_set() {
        let requested = ServerKey::new("/project/new", "rust");
        let only_idle = ServerKey::new("/project/only-idle", "rust");
        let mut entries = vec![admission_entry(
            &only_idle.project,
            &only_idle.language,
            true,
            true,
            0,
        )];
        entries.extend((0..MAX_LSP_SERVERS).map(|index| {
            admission_entry(
                &format!("/project/busy-{index}"),
                "rust",
                true,
                false,
                10 + index as u64,
            )
        }));

        // Seven live entries need two victims to leave a slot for the new
        // server. One idle candidate is insufficient, so nothing is selected.
        assert_eq!(
            plan_server_admission(&requested, entries).action,
            ServerStartAction::Reject
        );
    }

    #[test]
    fn typed_server_keys_and_project_stop_selection_are_collision_safe() {
        // Both pairs produced `a::b::c` with the old delimiter-composed key.
        let nested_project = ServerKey::new("b::c", "a");
        let delimiter_language = ServerKey::new("c", "a::b");
        assert_ne!(nested_project, delimiter_language);

        let suffix_project = ServerKey::new("folder::c", "rust");
        let mut registry = HashMap::new();
        registry.insert(nested_project.clone(), 1);
        registry.insert(delimiter_language.clone(), 2);
        registry.insert(suffix_project, 3);
        let mut selected = server_keys_for_project(&registry, "c");
        selected.sort();

        assert_eq!(selected, vec![delimiter_language]);
        assert!(registry.contains_key(&nested_project));
    }
}
