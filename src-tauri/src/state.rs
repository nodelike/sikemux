use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{ffi::ErrorCode, params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult};
use crate::observability::{global_observability, SpanOutcome};

const MAX_STATE_BYTES: usize = 32 * 1024 * 1024;
const MAX_ITEM_COUNT: usize = 4_096;
const MAX_ITEM_STATE_BYTES: usize = 1024 * 1024;
const MAX_ITEM_ID_BYTES: usize = 256;
const MAX_ITEM_KIND_BYTES: usize = 128;
const APPLICATION_STATE_VERSION: i64 = 15;
const CURRENT_SLOT: i64 = 0;
const BACKUP_SLOT: i64 = 1;
const RECOVERY_SNAPSHOT_ID: i64 = 1;
const DATABASE_SCHEMA_VERSION: i64 = 3;

#[derive(Clone, Debug)]
struct ItemStateRow {
    ordinal: i64,
    item_id: String,
    kind: String,
    version: i64,
    state_json: String,
}

#[derive(Debug)]
struct DecomposedSnapshot {
    schema_version: i64,
    core_json: String,
    items: Vec<ItemStateRow>,
}

#[derive(Debug)]
enum SlotLoad {
    Snapshot(String),
    Empty,
    Invalid,
}

#[derive(Debug)]
enum DatabaseLoad {
    Snapshot(String),
    Empty,
    Invalid,
}

#[derive(Debug)]
struct WriteDelta {
    core_changed: bool,
    changed_items: Vec<ItemStateRow>,
    removed_item_ids: Vec<String>,
}

/// Legacy JSON location retained as a one-way migration and recovery source.
/// Debug and release remain isolated so development cannot overwrite installed
/// application state.
pub fn state_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let filename = if cfg!(debug_assertions) {
        "state.dev.json"
    } else {
        "state.json"
    };
    Some(PathBuf::from(home).join(".config/sikemux").join(filename))
}

fn database_path() -> Option<PathBuf> {
    let legacy = state_path()?;
    let filename = if cfg!(debug_assertions) {
        "state.dev.sqlite3"
    } else {
        "state.sqlite3"
    };
    Some(legacy.with_file_name(filename))
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn migration_marker_path(database: &Path) -> PathBuf {
    sidecar_path(database, ".legacy-migrated")
}

fn write_migration_marker(database: &Path) -> AppResult<()> {
    let marker = migration_marker_path(database);
    fs::write(&marker, b"1\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(marker, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn quarantine_database(path: &Path) -> AppResult<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let quarantine = sidecar_path(path, &format!(".corrupt-{stamp}"));
    fs::rename(path, &quarantine)?;
    for suffix in ["-wal", "-shm"] {
        let source = sidecar_path(path, suffix);
        if source.exists() {
            let target = sidecar_path(&quarantine, suffix);
            fs::rename(source, target)?;
        }
    }
    Ok(quarantine)
}

/// Load the transactional SQLite snapshot, repairing it from the previous-good
/// recovery snapshot when necessary. Legacy JSON is considered only when no
/// authoritative database snapshot exists and migration has not been marked.
/// Perform the complete state load on a blocking thread.
///
/// SQLite recovery can wait for its bounded busy timeout and can also perform
/// migration, quarantine, and repair I/O. Keep this synchronous helper out of
/// Tauri's command executor; [`state_load`] and the combined boot command both
/// offload it through `spawn_blocking`.
pub(crate) fn state_load_sync() -> String {
    let observer = global_observability();
    let timer = observer.slow_operation(
        "state.sqlite_load",
        Duration::from_millis(50),
        None,
        Default::default(),
    );
    let loaded = if let (Some(database), Some(legacy)) = (database_path(), state_path()) {
        state_load_from_paths(&database, &legacy)
    } else {
        String::new()
    };
    let _ = observer.increment_counter("state.sqlite_loads", 1);
    observer.set_gauge("state.sqlite.last_loaded_bytes", loaded.len() as f64);
    timer.finish(SpanOutcome::Success);
    loaded
}

#[tauri::command]
pub async fn state_load() -> AppResult<String> {
    tauri::async_runtime::spawn_blocking(state_load_sync)
        .await
        .map_err(|error| AppError::Other(format!("state_load join: {error}")))
}

fn state_load_from_paths(database: &Path, legacy: &Path) -> String {
    if database.exists() {
        match database_user_version(database) {
            Ok(version) if version > DATABASE_SCHEMA_VERSION => return String::new(),
            Ok(_) => match load_database(database) {
                Ok(DatabaseLoad::Snapshot(snapshot)) => {
                    // A successful database load makes every surviving legacy
                    // file stale. Marker failures are retried by state_save and
                    // before any future quarantine.
                    let _ = mark_legacy_if_present(database, legacy);
                    return snapshot;
                }
                Ok(DatabaseLoad::Empty) => {}
                Ok(DatabaseLoad::Invalid) => {
                    if quarantine_authoritative_database(database, legacy).is_err() {
                        return String::new();
                    }
                    return String::new();
                }
                Err(_) => {
                    if !database_has_physical_corruption(database)
                        || quarantine_authoritative_database(database, legacy).is_err()
                    {
                        return String::new();
                    }
                    return String::new();
                }
            },
            Err(error) => {
                if !sqlite_error_is_corruption(&error)
                    && !database_has_physical_corruption(database)
                {
                    return String::new();
                }
                if quarantine_authoritative_database(database, legacy).is_err() {
                    return String::new();
                }
                return String::new();
            }
        }
    }
    migrate_legacy_snapshot(database, legacy)
}

fn legacy_snapshot_exists(legacy: &Path) -> bool {
    legacy.exists() || backup_path(legacy).exists()
}

fn mark_legacy_if_present(database: &Path, legacy: &Path) -> AppResult<()> {
    if legacy_snapshot_exists(legacy) && !migration_marker_path(database).is_file() {
        write_migration_marker(database)?;
    }
    Ok(())
}

fn quarantine_authoritative_database(database: &Path, legacy: &Path) -> AppResult<PathBuf> {
    close_database();
    // Establish the tombstone before moving an authoritative database. If the
    // marker cannot be persisted, leave the database in place so stale JSON
    // can never silently become authoritative on the next launch.
    mark_legacy_if_present(database, legacy)?;
    quarantine_database(database)
}

fn migrate_legacy_snapshot(database: &Path, legacy: &Path) -> String {
    // Once migration succeeded, stale JSON is retained only for manual
    // recovery and must never silently replace a newer corrupt database.
    if migration_marker_path(database).is_file() {
        return String::new();
    }
    let recovered = read_valid_json(legacy)
        .or_else(|| read_valid_json(&backup_path(legacy)))
        .unwrap_or_default();
    if !recovered.is_empty() {
        let observer = global_observability();
        match save_database(database, &recovered) {
            Ok(()) => {
                if write_migration_marker(database).is_err() {
                    let _ = observer.increment_counter("state.legacy_marker_failures", 1);
                }
            }
            Err(_) => {
                // The recovered payload is still the safest boot state. The
                // regular persistence retry can establish SQLite and its
                // migration marker without replacing the UI with defaults.
                let _ = observer.increment_counter("state.legacy_migration_failures", 1);
            }
        }
    }
    recovered
}

#[tauri::command]
pub async fn state_save(data: String) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || state_save_sync(data))
        .await
        .map_err(|error| AppError::Other(format!("state_save join: {error}")))?
}

fn state_save_sync(data: String) -> AppResult<()> {
    let path = database_path().ok_or_else(|| AppError::State("no home directory".into()))?;
    if let Some(legacy) = state_path() {
        return save_database_and_mark_legacy(&path, &legacy, &data);
    }
    save_database(&path, &data)
}

fn save_database_and_mark_legacy(path: &Path, legacy: &Path, data: &str) -> AppResult<()> {
    save_database(path, data)?;
    mark_legacy_if_present(path, legacy)
}

fn state_error(error: impl std::fmt::Display) -> AppError {
    AppError::State(error.to_string())
}

fn prepare_parent(path: &Path) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::State("invalid state database path".into()))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn secure_database_files(path: &Path) -> AppResult<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for candidate in [
            path.to_path_buf(),
            sidecar_path(path, "-wal"),
            sidecar_path(path, "-shm"),
        ] {
            if candidate.exists() {
                fs::set_permissions(candidate, fs::Permissions::from_mode(0o600))?;
            }
        }
    }
    Ok(())
}

fn database_cache() -> &'static Mutex<Option<CachedDatabase>> {
    static CACHE: OnceLock<Mutex<Option<CachedDatabase>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn lock_database_cache() -> std::sync::MutexGuard<'static, Option<CachedDatabase>> {
    database_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Drop the open handle. Anything that moves or replaces the database file has
/// to call this first: a handle kept across a rename goes on writing to a file
/// nobody will read again.
fn close_database() {
    *lock_database_cache() = None;
}

struct CachedDatabase {
    path: PathBuf,
    connection: Connection,
    /// SQLite creates `-wal` and `-shm` on the first write, so their
    /// permissions are set once after that rather than on every save.
    sidecars_secured: bool,
}

/// Run `work` against the process's one open state database.
///
/// Reopening it per save re-ran the whole schema and reset permissions on
/// every file, and a save lands every 600 ms while the user types. Any failure
/// drops the handle so the next attempt starts from a fresh open.
fn with_database<T>(
    path: &Path,
    work: impl FnOnce(&mut Connection) -> AppResult<T>,
) -> AppResult<T> {
    let mut cache = lock_database_cache();
    if cache.as_ref().is_none_or(|cached| cached.path != path) {
        *cache = Some(CachedDatabase {
            path: path.to_path_buf(),
            connection: open_database(path)?,
            sidecars_secured: false,
        });
    }
    let cached = cache.as_mut().expect("the database was just opened");
    let result = work(&mut cached.connection);
    match &result {
        Ok(_) => {
            if !cached.sidecars_secured && secure_database_files(path).is_ok() {
                cached.sidecars_secured = true;
            }
        }
        Err(_) => *cache = None,
    }
    result
}

fn open_database(path: &Path) -> AppResult<Connection> {
    prepare_parent(path)?;
    let mut connection = Connection::open(path).map_err(state_error)?;
    connection
        .busy_timeout(Duration::from_secs(3))
        .map_err(state_error)?;
    let user_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(state_error)?;
    if user_version > DATABASE_SCHEMA_VERSION {
        return Err(AppError::State(format!(
            "state database schema {user_version} is newer than supported schema {DATABASE_SCHEMA_VERSION}"
        )));
    }
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA wal_autocheckpoint = 256;
             CREATE TABLE IF NOT EXISTS workspace_snapshots (
                 slot INTEGER PRIMARY KEY CHECK (slot IN (0, 1)),
                 schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                 core_json TEXT NOT NULL,
                 saved_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS item_states (
                 slot INTEGER NOT NULL CHECK (slot IN (0, 1)),
                 ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
                 item_id TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 item_version INTEGER NOT NULL CHECK (item_version > 0),
                 state_json TEXT NOT NULL,
                 PRIMARY KEY (slot, item_id)
             );
             CREATE INDEX IF NOT EXISTS item_states_order
                 ON item_states(slot, ordinal);
             CREATE TABLE IF NOT EXISTS recovery_snapshots (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 snapshot_json TEXT NOT NULL,
                 saved_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_workspace_snapshot (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 schema_version INTEGER NOT NULL CHECK (schema_version > 0),
                 core_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_item_states (
                 item_id TEXT PRIMARY KEY,
                 removed INTEGER NOT NULL CHECK (removed IN (0, 1)),
                 ordinal INTEGER,
                 kind TEXT,
                 item_version INTEGER,
                 state_json TEXT,
                 CHECK (
                     (removed = 1 AND ordinal IS NULL AND kind IS NULL
                         AND item_version IS NULL AND state_json IS NULL)
                     OR
                     (removed = 0 AND ordinal >= 0 AND kind IS NOT NULL
                         AND item_version > 0 AND state_json IS NOT NULL)
                 )
             );",
        )
        .map_err(state_error)?;
    if user_version < DATABASE_SCHEMA_VERSION {
        migrate_database_schema(&mut connection, user_version)?;
    }
    secure_database_files(path)?;
    Ok(connection)
}

fn migrate_database_schema(connection: &mut Connection, from_version: i64) -> AppResult<()> {
    if !(0..DATABASE_SCHEMA_VERSION).contains(&from_version) {
        return Err(AppError::State(format!(
            "cannot migrate state database schema {from_version}"
        )));
    }

    let v1_backup_valid = from_version == 1
        && matches!(
            load_slot(connection, BACKUP_SLOT)?,
            SlotLoad::Snapshot(_) | SlotLoad::Empty
        );
    let v2_recovery = if from_version == 2 {
        match load_recovery_snapshot(connection)? {
            SlotLoad::Snapshot(snapshot) => Some(decompose_snapshot(&snapshot)?),
            SlotLoad::Empty | SlotLoad::Invalid => None,
        }
    } else {
        None
    };
    let transaction = connection.transaction().map_err(state_error)?;
    if let Some(snapshot) = v2_recovery {
        replace_slot_snapshot(&transaction, BACKUP_SLOT, &snapshot)?;
    } else if !v1_backup_valid {
        clear_slot(&transaction, BACKUP_SLOT)?;
    }
    seed_pending_generation(&transaction)?;
    transaction
        .execute("DELETE FROM recovery_snapshots", [])
        .map_err(state_error)?;
    transaction
        .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)
        .map_err(state_error)?;
    transaction.commit().map_err(state_error)
}

fn database_user_version(path: &Path) -> rusqlite::Result<i64> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn sqlite_error_is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(failure.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

fn database_has_physical_corruption(path: &Path) -> bool {
    let connection = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(connection) => connection,
        Err(error) => return sqlite_error_is_corruption(&error),
    };
    let result = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0));
    match result {
        Ok(result) => result != "ok",
        Err(error) => sqlite_error_is_corruption(&error),
    }
}

fn bounded_plain_string(value: &Value, max_bytes: usize) -> Option<&str> {
    let value = value.as_str()?;
    if value.is_empty()
        || value.len() > max_bytes
        || value.trim() != value
        || value.chars().any(char::is_control)
        || matches!(value, "__proto__" | "constructor" | "prototype")
    {
        return None;
    }
    Some(value)
}

fn decompose_snapshot(data: &str) -> AppResult<DecomposedSnapshot> {
    if data.len() > MAX_STATE_BYTES {
        return Err(AppError::State(
            "state snapshot exceeds 32 MiB limit".into(),
        ));
    }
    decompose_value(serde_json::from_str::<Value>(data)?)
}

/// Split a snapshot into the row shapes the database stores.
///
/// Item order is the order the keys appear in `itemStates`, which is the order
/// the user arranged them in. That is why `serde_json` is built with
/// `preserve_order`: a plain map would sort them by id and shuffle the
/// workspace on every reload.
fn decompose_value(mut root: Value) -> AppResult<DecomposedSnapshot> {
    let object = root
        .as_object_mut()
        .ok_or_else(|| AppError::State("state snapshot root must be an object".into()))?;
    let schema_version = object
        .get("version")
        .and_then(Value::as_i64)
        .filter(|version| *version > 0)
        .ok_or_else(|| AppError::State("state snapshot version is invalid".into()))?;
    let raw_items = object
        .remove("itemStates")
        .unwrap_or_else(|| Value::Object(Map::new()));
    let raw_items = raw_items
        .as_object()
        .ok_or_else(|| AppError::State("itemStates must be an object".into()))?;
    if raw_items.len() > MAX_ITEM_COUNT {
        return Err(AppError::State(
            "state snapshot exceeds 4096 item limit".into(),
        ));
    }

    let mut items = Vec::with_capacity(raw_items.len());
    for (ordinal, (map_key, envelope)) in raw_items.iter().enumerate() {
        let envelope = envelope
            .as_object()
            .filter(|value| value.len() == 4)
            .ok_or_else(|| AppError::State("item state envelope is malformed".into()))?;
        if !["itemId", "kind", "version", "state"]
            .iter()
            .all(|key| envelope.contains_key(*key))
        {
            return Err(AppError::State("item state envelope is malformed".into()));
        }
        let item_id = bounded_plain_string(
            envelope
                .get("itemId")
                .ok_or_else(|| AppError::State("item state id is missing".into()))?,
            MAX_ITEM_ID_BYTES,
        )
        .filter(|item_id| *item_id == map_key)
        .ok_or_else(|| AppError::State("item state id is invalid or mismatched".into()))?;
        let kind = bounded_plain_string(
            envelope
                .get("kind")
                .ok_or_else(|| AppError::State("item state kind is missing".into()))?,
            MAX_ITEM_KIND_BYTES,
        )
        .ok_or_else(|| AppError::State("item state kind is invalid".into()))?;
        let version = envelope
            .get("version")
            .and_then(Value::as_i64)
            .filter(|version| *version > 0)
            .ok_or_else(|| AppError::State("item state version is invalid".into()))?;
        let state_json = serde_json::to_string(
            envelope
                .get("state")
                .ok_or_else(|| AppError::State("item state payload is missing".into()))?,
        )?;
        if state_json.len() > MAX_ITEM_STATE_BYTES {
            return Err(AppError::State(
                "individual item state exceeds 1 MiB limit".into(),
            ));
        }
        items.push(ItemStateRow {
            ordinal: ordinal as i64,
            item_id: item_id.to_owned(),
            kind: kind.to_owned(),
            version,
            state_json,
        });
    }

    let core_json = serde_json::to_string(&root)?;
    Ok(DecomposedSnapshot {
        schema_version,
        core_json,
        items,
    })
}

fn save_database(path: &Path, data: &str) -> AppResult<()> {
    let snapshot = decompose_snapshot(data)?;
    if snapshot.schema_version != APPLICATION_STATE_VERSION {
        return Err(AppError::State(format!(
            "cannot save application state version {}; expected {APPLICATION_STATE_VERSION}",
            snapshot.schema_version
        )));
    }
    let observer = global_observability();
    let timer = observer.slow_operation(
        "state.sqlite_save",
        Duration::from_millis(16),
        None,
        Default::default(),
    );
    let delta = with_database(path, |connection| {
        let transaction = connection.transaction().map_err(state_error)?;
        let delta = write_transaction(&transaction, &snapshot)?;
        transaction.commit().map_err(state_error)?;
        Ok(delta)
    })?;
    let _ = observer.increment_counter("state.sqlite_saves", 1);
    let _ = observer.increment_counter(
        "state.sqlite.changed_items",
        delta.changed_items.len() as u64,
    );
    let _ = observer.increment_counter(
        "state.sqlite.removed_items",
        delta.removed_item_ids.len() as u64,
    );
    observer.set_gauge(
        "state.sqlite.last_delta_items",
        (delta.changed_items.len() + delta.removed_item_ids.len()) as f64,
    );
    observer.set_gauge("state.sqlite.last_item_count", snapshot.items.len() as f64);
    observer.set_gauge("state.sqlite.last_snapshot_bytes", data.len() as f64);
    timer.finish(SpanOutcome::Success);
    Ok(())
}

fn write_transaction(
    transaction: &Transaction<'_>,
    snapshot: &DecomposedSnapshot,
) -> AppResult<WriteDelta> {
    ensure_existing_state_is_compatible(transaction)?;
    advance_recovery_generation(transaction)?;
    let delta = write_current_snapshot(transaction, snapshot)?;
    store_pending_generation(transaction, snapshot, &delta)?;
    Ok(delta)
}

fn ensure_existing_state_is_compatible(transaction: &Transaction<'_>) -> AppResult<()> {
    let version = transaction
        .query_row(
            "SELECT MAX(schema_version) FROM (
                 SELECT schema_version FROM workspace_snapshots
                 UNION ALL
                 SELECT schema_version FROM pending_workspace_snapshot
             )",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(state_error)?;
    if version.is_some_and(|version| version > APPLICATION_STATE_VERSION) {
        return Err(AppError::State(format!(
            "existing application state is newer than supported version {APPLICATION_STATE_VERSION}"
        )));
    }
    Ok(())
}

fn unix_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn advance_recovery_generation(transaction: &Transaction<'_>) -> AppResult<()> {
    if !pending_generation_is_valid(transaction)? {
        return Err(AppError::State(
            "pending recovery generation is invalid".into(),
        ));
    }
    transaction
        .execute(
            "INSERT INTO workspace_snapshots(slot, schema_version, core_json, saved_at_ms)
             SELECT ?1, schema_version, core_json, ?2
             FROM pending_workspace_snapshot WHERE id = 1
             ON CONFLICT(slot) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 core_json = excluded.core_json,
                 saved_at_ms = excluded.saved_at_ms",
            params![BACKUP_SLOT, unix_time_millis()],
        )
        .map_err(state_error)?;
    transaction
        .execute(
            "INSERT INTO item_states(slot, ordinal, item_id, kind, item_version, state_json)
             SELECT ?1, ordinal, item_id, kind, item_version, state_json
             FROM pending_item_states WHERE removed = 0
             ON CONFLICT(slot, item_id) DO UPDATE SET
                 ordinal = excluded.ordinal,
                 kind = excluded.kind,
                 item_version = excluded.item_version,
                 state_json = excluded.state_json",
            params![BACKUP_SLOT],
        )
        .map_err(state_error)?;
    transaction
        .execute(
            "DELETE FROM item_states
             WHERE slot = ?1 AND item_id IN (
                 SELECT item_id FROM pending_item_states WHERE removed = 1
             )",
            params![BACKUP_SLOT],
        )
        .map_err(state_error)?;
    transaction
        .execute("DELETE FROM pending_workspace_snapshot", [])
        .map_err(state_error)?;
    transaction
        .execute("DELETE FROM pending_item_states", [])
        .map_err(state_error)?;
    Ok(())
}

fn write_current_snapshot(
    transaction: &Transaction<'_>,
    snapshot: &DecomposedSnapshot,
) -> AppResult<WriteDelta> {
    let core_changed = transaction
        .execute(
            "INSERT INTO workspace_snapshots(slot, schema_version, core_json, saved_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(slot) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 core_json = excluded.core_json,
                 saved_at_ms = excluded.saved_at_ms
             WHERE schema_version IS NOT excluded.schema_version
                OR core_json IS NOT excluded.core_json",
            params![
                CURRENT_SLOT,
                snapshot.schema_version,
                snapshot.core_json,
                unix_time_millis()
            ],
        )
        .map_err(state_error)?
        > 0;
    let mut existing_statement = transaction
        .prepare("SELECT item_id FROM item_states WHERE slot = ?1")
        .map_err(state_error)?;
    let mut stale_item_ids = existing_statement
        .query_map(params![CURRENT_SLOT], |row| row.get::<_, String>(0))
        .map_err(state_error)?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(state_error)?;
    drop(existing_statement);

    let mut statement = transaction
        .prepare_cached(
            "INSERT INTO item_states(slot, ordinal, item_id, kind, item_version, state_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(slot, item_id) DO UPDATE SET
                 ordinal = excluded.ordinal,
                 kind = excluded.kind,
                 item_version = excluded.item_version,
                 state_json = excluded.state_json
             WHERE ordinal IS NOT excluded.ordinal
                OR kind IS NOT excluded.kind
                OR item_version IS NOT excluded.item_version
                OR state_json IS NOT excluded.state_json",
        )
        .map_err(state_error)?;
    let mut changed_items = Vec::new();
    for item in &snapshot.items {
        let changed = statement
            .execute(params![
                CURRENT_SLOT,
                item.ordinal,
                item.item_id,
                item.kind,
                item.version,
                item.state_json
            ])
            .map_err(state_error)?;
        if changed > 0 {
            changed_items.push(item.clone());
        }
        stale_item_ids.remove(&item.item_id);
    }
    drop(statement);
    let mut removed_item_ids = stale_item_ids.into_iter().collect::<Vec<_>>();
    removed_item_ids.sort_unstable();
    let mut delete_statement = transaction
        .prepare_cached("DELETE FROM item_states WHERE slot = ?1 AND item_id = ?2")
        .map_err(state_error)?;
    for item_id in &removed_item_ids {
        delete_statement
            .execute(params![CURRENT_SLOT, item_id])
            .map_err(state_error)?;
    }
    Ok(WriteDelta {
        core_changed,
        changed_items,
        removed_item_ids,
    })
}

fn store_pending_generation(
    transaction: &Transaction<'_>,
    snapshot: &DecomposedSnapshot,
    delta: &WriteDelta,
) -> AppResult<()> {
    if delta.core_changed {
        transaction
            .execute(
                "INSERT INTO pending_workspace_snapshot(id, schema_version, core_json)
                 VALUES (1, ?1, ?2)",
                params![snapshot.schema_version, snapshot.core_json],
            )
            .map_err(state_error)?;
    }
    let mut changed_statement = transaction
        .prepare_cached(
            "INSERT INTO pending_item_states(
                 item_id, removed, ordinal, kind, item_version, state_json
             ) VALUES (?1, 0, ?2, ?3, ?4, ?5)",
        )
        .map_err(state_error)?;
    for item in &delta.changed_items {
        changed_statement
            .execute(params![
                item.item_id,
                item.ordinal,
                item.kind,
                item.version,
                item.state_json
            ])
            .map_err(state_error)?;
    }
    drop(changed_statement);
    let mut removed_statement = transaction
        .prepare_cached(
            "INSERT INTO pending_item_states(item_id, removed)
             VALUES (?1, 1)",
        )
        .map_err(state_error)?;
    for item_id in &delta.removed_item_ids {
        removed_statement
            .execute(params![item_id])
            .map_err(state_error)?;
    }
    Ok(())
}

fn load_database(path: &Path) -> AppResult<DatabaseLoad> {
    with_database(path, load_from_connection)
}

fn load_from_connection(connection: &mut Connection) -> AppResult<DatabaseLoad> {
    let current = load_slot(connection, CURRENT_SLOT)?;
    if let SlotLoad::Snapshot(snapshot) = &current {
        return Ok(DatabaseLoad::Snapshot(snapshot.clone()));
    }

    // The pending delta was validated before its original commit. Reapply it
    // to the previous generation inside a transaction, validate the assembled
    // result, and only then promote it over an invalid live snapshot.
    let pending_valid = pending_generation_is_valid(connection)?;
    if pending_valid {
        let transaction = connection.transaction().map_err(state_error)?;
        advance_recovery_generation(&transaction)?;
        if let SlotLoad::Snapshot(snapshot) = load_slot(&transaction, BACKUP_SLOT)? {
            let decomposed = decompose_snapshot(&snapshot)?;
            replace_slot_snapshot(&transaction, CURRENT_SLOT, &decomposed)?;
            transaction.commit().map_err(state_error)?;
            return Ok(DatabaseLoad::Snapshot(snapshot));
        }
    }

    let recovery = load_slot(connection, BACKUP_SLOT)?;
    if let SlotLoad::Snapshot(snapshot) = &recovery {
        let decomposed = decompose_snapshot(snapshot)?;
        let transaction = connection.transaction().map_err(state_error)?;
        replace_slot_snapshot(&transaction, CURRENT_SLOT, &decomposed)?;
        transaction
            .execute("DELETE FROM pending_workspace_snapshot", [])
            .map_err(state_error)?;
        transaction
            .execute("DELETE FROM pending_item_states", [])
            .map_err(state_error)?;
        transaction.commit().map_err(state_error)?;
        return Ok(DatabaseLoad::Snapshot(snapshot.clone()));
    }

    Ok(match (current, recovery, pending_valid) {
        (SlotLoad::Empty, SlotLoad::Empty, true) => DatabaseLoad::Empty,
        _ => DatabaseLoad::Invalid,
    })
}

fn load_recovery_snapshot(connection: &Connection) -> AppResult<SlotLoad> {
    let snapshot = connection
        .query_row(
            "SELECT snapshot_json FROM recovery_snapshots WHERE id = ?1",
            params![RECOVERY_SNAPSHOT_ID],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(state_error)?;
    let Some(snapshot) = snapshot else {
        return Ok(SlotLoad::Empty);
    };
    if snapshot.len() > MAX_STATE_BYTES || decompose_snapshot(&snapshot).is_err() {
        return Ok(SlotLoad::Invalid);
    }
    Ok(SlotLoad::Snapshot(snapshot))
}

fn pending_generation_is_valid(connection: &Connection) -> AppResult<bool> {
    let core = connection
        .query_row(
            "SELECT schema_version, core_json
             FROM pending_workspace_snapshot WHERE id = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(state_error)?;
    if let Some((schema_version, core_json)) = core {
        if core_json.len() > MAX_STATE_BYTES {
            return Ok(false);
        }
        let root = match serde_json::from_str::<Value>(&core_json) {
            Ok(Value::Object(root)) => root,
            _ => return Ok(false),
        };
        if schema_version <= 0
            || root.get("version").and_then(Value::as_i64) != Some(schema_version)
            || root.contains_key("itemStates")
        {
            return Ok(false);
        }
    }

    let mut statement = connection
        .prepare(
            "SELECT item_id, removed, ordinal, kind, item_version, state_json
             FROM pending_item_states",
        )
        .map_err(state_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(state_error)?;
    let mut count = 0usize;
    for row in rows {
        count += 1;
        if count > MAX_ITEM_COUNT * 2 {
            return Ok(false);
        }
        let (item_id, removed, ordinal, kind, version, state_json) = row.map_err(state_error)?;
        if bounded_plain_string(&Value::String(item_id), MAX_ITEM_ID_BYTES).is_none() {
            return Ok(false);
        }
        match (removed, ordinal, kind, version, state_json) {
            (1, None, None, None, None) => {}
            (0, Some(ordinal), Some(kind), Some(version), Some(state_json))
                if ordinal >= 0
                    && ordinal < MAX_ITEM_COUNT as i64
                    && bounded_plain_string(&Value::String(kind.clone()), MAX_ITEM_KIND_BYTES)
                        .is_some()
                    && version > 0
                    && state_json.len() <= MAX_ITEM_STATE_BYTES
                    && serde_json::from_str::<Value>(&state_json).is_ok() => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn clear_slot(transaction: &Transaction<'_>, slot: i64) -> AppResult<()> {
    transaction
        .execute("DELETE FROM item_states WHERE slot = ?1", params![slot])
        .map_err(state_error)?;
    transaction
        .execute(
            "DELETE FROM workspace_snapshots WHERE slot = ?1",
            params![slot],
        )
        .map_err(state_error)?;
    Ok(())
}

fn replace_slot_snapshot(
    transaction: &Transaction<'_>,
    slot: i64,
    snapshot: &DecomposedSnapshot,
) -> AppResult<()> {
    clear_slot(transaction, slot)?;
    transaction
        .execute(
            "INSERT INTO workspace_snapshots(slot, schema_version, core_json, saved_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                slot,
                snapshot.schema_version,
                snapshot.core_json,
                unix_time_millis()
            ],
        )
        .map_err(state_error)?;
    let mut statement = transaction
        .prepare_cached(
            "INSERT INTO item_states(slot, ordinal, item_id, kind, item_version, state_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(state_error)?;
    for item in &snapshot.items {
        statement
            .execute(params![
                slot,
                item.ordinal,
                item.item_id,
                item.kind,
                item.version,
                item.state_json
            ])
            .map_err(state_error)?;
    }
    Ok(())
}

fn seed_pending_generation(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction
        .execute("DELETE FROM pending_workspace_snapshot", [])
        .map_err(state_error)?;
    transaction
        .execute("DELETE FROM pending_item_states", [])
        .map_err(state_error)?;
    transaction
        .execute(
            "INSERT INTO pending_workspace_snapshot(id, schema_version, core_json)
             SELECT 1, current.schema_version, current.core_json
             FROM workspace_snapshots AS current
             LEFT JOIN workspace_snapshots AS backup ON backup.slot = ?2
             WHERE current.slot = ?1
               AND (backup.slot IS NULL
                    OR current.schema_version IS NOT backup.schema_version
                    OR current.core_json IS NOT backup.core_json)",
            params![CURRENT_SLOT, BACKUP_SLOT],
        )
        .map_err(state_error)?;
    transaction
        .execute(
            "INSERT INTO pending_item_states(
                 item_id, removed, ordinal, kind, item_version, state_json
             )
             SELECT current.item_id, 0, current.ordinal, current.kind,
                    current.item_version, current.state_json
             FROM item_states AS current
             LEFT JOIN item_states AS backup
               ON backup.slot = ?2 AND backup.item_id = current.item_id
             WHERE current.slot = ?1
               AND (backup.item_id IS NULL
                    OR current.ordinal IS NOT backup.ordinal
                    OR current.kind IS NOT backup.kind
                    OR current.item_version IS NOT backup.item_version
                    OR current.state_json IS NOT backup.state_json)",
            params![CURRENT_SLOT, BACKUP_SLOT],
        )
        .map_err(state_error)?;
    transaction
        .execute(
            "INSERT INTO pending_item_states(item_id, removed)
             SELECT backup.item_id, 1
             FROM item_states AS backup
             LEFT JOIN item_states AS current
               ON current.slot = ?1 AND current.item_id = backup.item_id
             WHERE backup.slot = ?2 AND current.item_id IS NULL",
            params![CURRENT_SLOT, BACKUP_SLOT],
        )
        .map_err(state_error)?;
    Ok(())
}

fn load_slot(connection: &Connection, slot: i64) -> AppResult<SlotLoad> {
    let snapshot = connection
        .query_row(
            "SELECT schema_version, core_json FROM workspace_snapshots WHERE slot = ?1",
            params![slot],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(state_error)?;
    let Some((schema_version, core_json)) = snapshot else {
        return Ok(SlotLoad::Empty);
    };
    if core_json.len() > MAX_STATE_BYTES {
        return Ok(SlotLoad::Invalid);
    }
    let mut root = match serde_json::from_str::<Value>(&core_json) {
        Ok(Value::Object(root)) => root,
        _ => return Ok(SlotLoad::Invalid),
    };
    if root.get("version").and_then(Value::as_i64) != Some(schema_version)
        || root.contains_key("itemStates")
    {
        return Ok(SlotLoad::Invalid);
    }
    let mut statement = connection
        .prepare(
            "SELECT ordinal, item_id, kind, item_version, state_json
             FROM item_states WHERE slot = ?1 ORDER BY ordinal ASC",
        )
        .map_err(state_error)?;
    let rows = statement
        .query_map(params![slot], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(state_error)?;
    let mut item_states = Map::new();
    for row in rows {
        let (ordinal, item_id, kind, version, state_json) = row.map_err(state_error)?;
        if item_states.len() >= MAX_ITEM_COUNT
            || ordinal != item_states.len() as i64
            || bounded_plain_string(&Value::String(item_id.clone()), MAX_ITEM_ID_BYTES).is_none()
            || bounded_plain_string(&Value::String(kind.clone()), MAX_ITEM_KIND_BYTES).is_none()
            || version <= 0
            || state_json.len() > MAX_ITEM_STATE_BYTES
        {
            return Ok(SlotLoad::Invalid);
        }
        let state = match serde_json::from_str::<Value>(&state_json) {
            Ok(state) => state,
            Err(_) => return Ok(SlotLoad::Invalid),
        };
        item_states.insert(
            item_id.clone(),
            Value::Object(Map::from_iter([
                ("itemId".to_owned(), Value::String(item_id)),
                ("kind".to_owned(), Value::String(kind)),
                ("version".to_owned(), Value::Number(version.into())),
                ("state".to_owned(), state),
            ])),
        );
    }
    root.insert("itemStates".to_owned(), Value::Object(item_states));
    let root = Value::Object(root);
    let snapshot = serde_json::to_string(&root)?;
    // Validate the assembled value directly. Handing the serialized form back
    // to `decompose_snapshot` parsed the whole state a second time, and this
    // runs on the boot path.
    if snapshot.len() > MAX_STATE_BYTES || decompose_value(root).is_err() {
        return Ok(SlotLoad::Invalid);
    }
    Ok(SlotLoad::Snapshot(snapshot))
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

fn read_valid_json(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_STATE_BYTES {
        return None;
    }
    let data = String::from_utf8(bytes).ok()?;
    serde_json::from_str::<Value>(&data).ok()?;
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(theme: &str, items: &str) -> String {
        format!(
            r#"{{"version":{APPLICATION_STATE_VERSION},"theme":"{theme}","itemStates":{items}}}"#
        )
    }

    fn expect_snapshot(load: DatabaseLoad) -> String {
        match load {
            DatabaseLoad::Snapshot(snapshot) => snapshot,
            other => panic!("expected snapshot, got {other:?}"),
        }
    }

    fn expect_slot_snapshot(load: SlotLoad) -> String {
        match load {
            SlotLoad::Snapshot(snapshot) => snapshot,
            other => panic!("expected slot snapshot, got {other:?}"),
        }
    }

    fn theme(snapshot: &str) -> String {
        serde_json::from_str::<Value>(snapshot).unwrap()["theme"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn sqlite_round_trip_separates_and_reassembles_item_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let data = snapshot(
            "dark",
            r#"{"editor-1":{"itemId":"editor-1","kind":"editor","version":1,"state":{"openTabs":["/a"]}}}"#,
        );

        save_database(&path, &data).unwrap();
        let loaded = expect_snapshot(load_database(&path).unwrap());
        assert_eq!(loaded, data);

        let connection = Connection::open(&path).unwrap();
        let core: String = connection
            .query_row(
                "SELECT core_json FROM workspace_snapshots WHERE slot = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!core.contains("editor-1"));
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM item_states WHERE slot = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn each_commit_keeps_previous_good_rows_and_only_journals_the_new_delta() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let first = snapshot("first", "{}");
        let second = snapshot("second", "{}");
        save_database(&path, &first).unwrap();
        save_database(&path, &second).unwrap();

        let connection = open_database(&path).unwrap();
        let current = expect_slot_snapshot(load_slot(&connection, CURRENT_SLOT).unwrap());
        let recovery = expect_slot_snapshot(load_slot(&connection, BACKUP_SLOT).unwrap());
        assert_eq!(theme(&current), "second");
        assert_eq!(theme(&recovery), "first");
        let pending_core: String = connection
            .query_row(
                "SELECT core_json FROM pending_workspace_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&pending_core).unwrap()["theme"],
            "second"
        );
        assert!(matches!(
            load_recovery_snapshot(&connection).unwrap(),
            SlotLoad::Empty
        ));
    }

    #[test]
    fn unchanged_items_are_not_rewritten_and_removed_items_delete_individually() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let with_item = snapshot(
            "first",
            r#"{"editor":{"itemId":"editor","kind":"editor","version":1,"state":{"value":1}}}"#,
        );
        save_database(&path, &with_item).unwrap();

        let mut connection = open_database(&path).unwrap();
        let unchanged = decompose_snapshot(&snapshot(
            "second",
            r#"{"editor":{"itemId":"editor","kind":"editor","version":1,"state":{"value":1}}}"#,
        ))
        .unwrap();
        let transaction = connection.transaction().unwrap();
        let delta = write_transaction(&transaction, &unchanged).unwrap();
        assert!(delta.core_changed);
        assert!(delta.changed_items.is_empty());
        assert!(delta.removed_item_ids.is_empty());
        transaction.commit().unwrap();

        let without_item = decompose_snapshot(&snapshot("third", "{}")).unwrap();
        let transaction = connection.transaction().unwrap();
        let delta = write_transaction(&transaction, &without_item).unwrap();
        assert!(delta.core_changed);
        assert!(delta.changed_items.is_empty());
        assert_eq!(delta.removed_item_ids, ["editor"]);
        transaction.commit().unwrap();
        assert_eq!(
            theme(&expect_snapshot(load_database(&path).unwrap())),
            "third"
        );
    }

    #[test]
    fn malformed_or_oversized_item_envelopes_are_rejected_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let valid = snapshot("safe", "{}");
        save_database(&path, &valid).unwrap();

        let mismatched = snapshot(
            "unsafe",
            r#"{"key":{"itemId":"other","kind":"editor","version":1,"state":null}}"#,
        );
        assert!(save_database(&path, &mismatched).is_err());
        let loaded = expect_snapshot(load_database(&path).unwrap());
        assert_eq!(theme(&loaded), "safe");

        let oversized_state = "x".repeat(MAX_ITEM_STATE_BYTES + 1);
        let oversized = snapshot(
            "unsafe",
            &format!(
                r#"{{"key":{{"itemId":"key","kind":"editor","version":1,"state":"{oversized_state}"}}}}"#
            ),
        );
        assert!(save_database(&path, &oversized).is_err());
    }

    #[test]
    fn future_application_state_cannot_be_saved_or_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        save_database(&path, &snapshot("safe", "{}")).unwrap();
        let future_version = APPLICATION_STATE_VERSION + 1;
        let future =
            format!(r#"{{"version":{future_version},"theme":"future","itemStates":{{}}}}"#);

        assert!(save_database(&path, &future).is_err());
        assert_eq!(
            theme(&expect_snapshot(load_database(&path).unwrap())),
            "safe"
        );

        let future_snapshot = decompose_snapshot(&future).unwrap();
        let mut connection = open_database(&path).unwrap();
        let transaction = connection.transaction().unwrap();
        replace_slot_snapshot(&transaction, CURRENT_SLOT, &future_snapshot).unwrap();
        transaction.commit().unwrap();
        drop(connection);

        assert!(save_database(&path, &snapshot("downgrade", "{}")).is_err());
        assert_eq!(
            theme(&expect_snapshot(load_database(&path).unwrap())),
            "future"
        );
    }

    #[test]
    fn corrupt_current_is_repaired_from_recovery_before_the_next_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        save_database(&path, &snapshot("backup", "{}")).unwrap();
        save_database(&path, &snapshot("current", "{}")).unwrap();

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE workspace_snapshots SET core_json = '{' WHERE slot = 0",
                [],
            )
            .unwrap();
        drop(connection);
        let loaded = expect_snapshot(load_database(&path).unwrap());
        assert_eq!(theme(&loaded), "current");

        let connection = open_database(&path).unwrap();
        let repaired = expect_slot_snapshot(load_slot(&connection, CURRENT_SLOT).unwrap());
        assert_eq!(theme(&repaired), "current");
        drop(connection);

        save_database(&path, &snapshot("next", "{}")).unwrap();
        let connection = open_database(&path).unwrap();
        let recovery = expect_slot_snapshot(load_slot(&connection, BACKUP_SLOT).unwrap());
        assert_eq!(theme(&recovery), "current");
    }

    #[test]
    fn legacy_migration_is_marked_and_stale_json_cannot_resurrect() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite3");
        let legacy = directory.path().join("state.json");
        let legacy_snapshot = snapshot("legacy", "{}");
        fs::write(&legacy, &legacy_snapshot).unwrap();

        assert_eq!(migrate_legacy_snapshot(&database, &legacy), legacy_snapshot);
        assert!(database.exists());
        assert!(migration_marker_path(&database).exists());
        assert_eq!(
            database_user_version(&database).unwrap(),
            DATABASE_SCHEMA_VERSION
        );

        fs::write(&legacy, snapshot("stale", "{}")).unwrap();
        close_database();
        fs::write(&database, b"not a database").unwrap();
        assert!(state_load_from_paths(&database, &legacy).is_empty());
        assert!(!database.exists());
        assert!(migration_marker_path(&database).is_file());
        assert!(state_load_from_paths(&database, &legacy).is_empty());
    }

    #[test]
    fn future_database_schema_is_preserved_and_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);

        assert_eq!(
            database_user_version(&path).unwrap(),
            DATABASE_SCHEMA_VERSION + 1
        );
        assert!(open_database(&path).is_err());
        assert!(path.exists());
    }

    #[test]
    fn schema_one_backup_rows_migrate_to_the_incremental_journal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE workspace_snapshots (
                     slot INTEGER PRIMARY KEY CHECK (slot IN (0, 1)),
                     schema_version INTEGER NOT NULL,
                     core_json TEXT NOT NULL,
                     saved_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE item_states (
                     slot INTEGER NOT NULL CHECK (slot IN (0, 1)),
                     ordinal INTEGER NOT NULL,
                     item_id TEXT NOT NULL,
                     kind TEXT NOT NULL,
                     item_version INTEGER NOT NULL,
                     state_json TEXT NOT NULL,
                     PRIMARY KEY (slot, item_id)
                 );
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        let current = decompose_snapshot(&snapshot("current", "{}")).unwrap();
        let backup = decompose_snapshot(&snapshot(
            "backup",
            r#"{"editor":{"itemId":"editor","kind":"editor","version":1,"state":{"value":1}}}"#,
        ))
        .unwrap();
        let transaction = connection.transaction().unwrap();
        write_current_snapshot(&transaction, &current).unwrap();
        transaction
            .execute(
                "INSERT INTO workspace_snapshots(slot, schema_version, core_json, saved_at_ms)
                 VALUES (?1, ?2, ?3, 1)",
                params![BACKUP_SLOT, backup.schema_version, backup.core_json],
            )
            .unwrap();
        for item in &backup.items {
            transaction
                .execute(
                    "INSERT INTO item_states(slot, ordinal, item_id, kind, item_version, state_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        BACKUP_SLOT,
                        item.ordinal,
                        item.item_id,
                        item.kind,
                        item.version,
                        item.state_json
                    ],
                )
                .unwrap();
        }
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_database(&path).unwrap();
        assert_eq!(
            database_user_version(&path).unwrap(),
            DATABASE_SCHEMA_VERSION
        );
        let recovery = expect_slot_snapshot(load_slot(&connection, BACKUP_SLOT).unwrap());
        assert_eq!(theme(&recovery), "backup");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM item_states WHERE slot = ?1",
                    params![BACKUP_SLOT],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pending_workspace_snapshot",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn schema_two_recovery_blob_migrates_to_recovery_rows_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE workspace_snapshots (
                     slot INTEGER PRIMARY KEY CHECK (slot IN (0, 1)),
                     schema_version INTEGER NOT NULL,
                     core_json TEXT NOT NULL,
                     saved_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE item_states (
                     slot INTEGER NOT NULL CHECK (slot IN (0, 1)),
                     ordinal INTEGER NOT NULL,
                     item_id TEXT NOT NULL,
                     kind TEXT NOT NULL,
                     item_version INTEGER NOT NULL,
                     state_json TEXT NOT NULL,
                     PRIMARY KEY (slot, item_id)
                 );
                 CREATE TABLE recovery_snapshots (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     snapshot_json TEXT NOT NULL,
                     saved_at_ms INTEGER NOT NULL
                 );
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        let current = decompose_snapshot(&snapshot("current", "{}")).unwrap();
        let recovery = snapshot(
            "recovery",
            r#"{"editor":{"itemId":"editor","kind":"editor","version":1,"state":{"value":1}}}"#,
        );
        let transaction = connection.transaction().unwrap();
        write_current_snapshot(&transaction, &current).unwrap();
        transaction
            .execute(
                "INSERT INTO recovery_snapshots(id, snapshot_json, saved_at_ms)
                 VALUES (1, ?1, 1)",
                params![recovery],
            )
            .unwrap();
        transaction.commit().unwrap();
        drop(connection);

        let connection = open_database(&path).unwrap();
        let migrated = expect_slot_snapshot(load_slot(&connection, BACKUP_SLOT).unwrap());
        assert_eq!(theme(&migrated), "recovery");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM recovery_snapshots", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert!(pending_generation_is_valid(&connection).unwrap());
    }

    #[test]
    fn recovery_advances_only_the_previous_item_delta() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.sqlite3");
        let first_items = r#"{
            "a":{"itemId":"a","kind":"editor","version":1,"state":{"value":1}},
            "b":{"itemId":"b","kind":"editor","version":1,"state":{"value":1}}
        }"#;
        let changed_a = r#"{
            "a":{"itemId":"a","kind":"editor","version":1,"state":{"value":2}},
            "b":{"itemId":"b","kind":"editor","version":1,"state":{"value":1}}
        }"#;
        let changed_b = r#"{
            "a":{"itemId":"a","kind":"editor","version":1,"state":{"value":2}},
            "b":{"itemId":"b","kind":"editor","version":1,"state":{"value":2}}
        }"#;
        save_database(&path, &snapshot("same", first_items)).unwrap();
        save_database(&path, &snapshot("same", first_items)).unwrap();

        let connection = open_database(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM pending_item_states", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        drop(connection);

        save_database(&path, &snapshot("same", changed_a)).unwrap();
        let connection = open_database(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT item_id FROM pending_item_states", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "a"
        );
        let backup_before: String = connection
            .query_row(
                "SELECT state_json FROM item_states WHERE slot = ?1 AND item_id = 'a'",
                params![BACKUP_SLOT],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&backup_before).unwrap()["value"],
            1
        );
        drop(connection);

        save_database(&path, &snapshot("same", changed_b)).unwrap();
        let connection = open_database(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT item_id FROM pending_item_states", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "b"
        );
        let backup_after: String = connection
            .query_row(
                "SELECT state_json FROM item_states WHERE slot = ?1 AND item_id = 'a'",
                params![BACKUP_SLOT],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&backup_after).unwrap()["value"],
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM recovery_snapshots", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    /// The order of the keys in `itemStates` is the order the user arranged
    /// their panes in, and it survives a save and a load. This is what the
    /// `preserve_order` feature on `serde_json` buys: a sorted map would
    /// renumber them by id and shuffle the workspace on every launch.
    #[test]
    fn item_order_survives_a_round_trip_in_the_order_the_user_left_it() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite3");
        let items = r#"{"zebra":{"itemId":"zebra","kind":"pane","version":1,"state":{}},"alpha":{"itemId":"alpha","kind":"pane","version":1,"state":{}}}"#;

        save_database(&database, &snapshot("current", items)).unwrap();
        let loaded = expect_snapshot(load_database(&database).unwrap());

        let root = serde_json::from_str::<Value>(&loaded).unwrap();
        let order = root["itemStates"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["zebra".to_string(), "alpha".to_string()]);
    }

    #[test]
    fn busy_database_is_preserved_instead_of_quarantined() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite3");
        let legacy = directory.path().join("state.json");
        save_database(&database, &snapshot("current", "{}")).unwrap();
        // Stand in for another process owning the file.
        close_database();
        let lock = Connection::open(&database).unwrap();
        lock.execute_batch("PRAGMA journal_mode = DELETE; BEGIN EXCLUSIVE;")
            .unwrap();

        assert!(state_load_from_paths(&database, &legacy).is_empty());
        assert!(database.exists());
        let quarantines = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
            .count();
        assert_eq!(quarantines, 0);
        lock.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn marker_failures_are_reported_and_retried_after_database_commit() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite3");
        let legacy = directory.path().join("state.json");
        fs::write(&legacy, snapshot("legacy", "{}")).unwrap();
        let marker = migration_marker_path(&database);
        fs::create_dir(&marker).unwrap();

        assert!(
            save_database_and_mark_legacy(&database, &legacy, &snapshot("current", "{}")).is_err()
        );
        assert_eq!(
            theme(&expect_snapshot(load_database(&database).unwrap())),
            "current"
        );

        fs::remove_dir(&marker).unwrap();
        save_database_and_mark_legacy(&database, &legacy, &snapshot("current", "{}")).unwrap();
        assert!(marker.is_file());
    }

    #[test]
    fn migration_returns_recovered_state_when_the_marker_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state.sqlite3");
        let legacy = directory.path().join("state.json");
        let recovered = snapshot("recovered", "{}");
        fs::write(&legacy, &recovered).unwrap();
        fs::create_dir(migration_marker_path(&database)).unwrap();

        assert_eq!(migrate_legacy_snapshot(&database, &legacy), recovered);
        assert_eq!(
            theme(&expect_snapshot(load_database(&database).unwrap())),
            "recovered"
        );
    }

    #[test]
    fn invalid_legacy_primary_can_fall_back_to_json_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, "{").unwrap();
        fs::write(backup_path(&path), snapshot("backup", "{}")).unwrap();
        assert!(read_valid_json(&path).is_none());
        assert!(read_valid_json(&backup_path(&path)).is_some());
    }
}
