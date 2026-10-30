//! What the Database plugin's views hand a pane: the saved databases as the explorer shows
//! them, one table's structure, and the queries run on a database.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Postgres,
    Mysql,
    Sqlite,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

/// Every saved database, with what the explorer has open in each.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Explorer {
    pub connections: Vec<Connection>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub engine: Engine,
    /// The engine as people name it, such as PostgreSQL.
    pub engine_name: String,
    /// Where it is, as the person would recognise it: `app@localhost:5432/shop`, or the file's name.
    pub address: String,
    pub read_only: bool,
    /// The engine and version it is connected to, such as `PostgreSQL 16.4`; none while it is not.
    pub connected: Option<String>,
    /// The schema it opens on: `public`, `main`, or the database a MySQL one names.
    pub default_schema: String,
    /// What its properties page lists, in order.
    pub facts: Vec<Fact>,
    /// Its schemas, while it is connected and open in the explorer.
    pub schemas: Option<Vec<Schema>>,
    /// Each database on a PostgreSQL server saved without one, as a connection of its own, while
    /// it is connected and open in the explorer.
    pub databases: Option<Vec<Connection>>,
    /// Why its schemas could not be read.
    pub failed: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub label: String,
    pub value: String,
    /// Written as code, such as an address or a path.
    pub mono: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub name: String,
    /// Its tables, then its views, while it is open in the explorer.
    pub groups: Option<Vec<TableGroup>>,
    /// Why its tables could not be read.
    pub failed: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableGroup {
    /// `tables` or `views`.
    pub name: String,
    pub tables: Vec<TableEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableEntry {
    pub name: String,
    pub kind: TableKind,
    /// Its kind in words, such as "materialized view".
    pub kind_name: String,
    /// A query for its first rows, to start from.
    pub preview: String,
    /// Its columns, while it is open in the explorer.
    pub columns: Option<Vec<Column>>,
    /// Why its columns could not be read.
    pub failed: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
}

/// One table's or view's structure.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableView {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    pub kind_name: String,
    /// A query for its first rows, to start from.
    pub preview: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<IndexRow>,
    pub foreign_keys: Vec<ForeignKeyRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexRow {
    pub name: String,
    /// Its columns and what it is, such as `(email) unique`.
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyRow {
    pub columns: String,
    /// The table it points at, to open it.
    pub schema: String,
    pub table: String,
    /// What it points at, as `users(id)`.
    pub target: String,
}

/// The queries run on one database, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    /// What the list says when it has none.
    pub empty: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub sql: String,
    /// Milliseconds since 1970.
    pub at: u64,
    pub millis: u64,
    pub ok: bool,
    /// Rows the last statement returned or changed.
    pub rows: Option<u64>,
    pub error: Option<String>,
    pub by_agent: bool,
}

/// What one run of SQL gave: a result for each statement, in the order they ran.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryOutcome {
    pub results: Vec<ResultSet>,
    /// The result shown first: the last that returned rows, or else the last.
    pub main: usize,
    pub millis: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<ResultColumn>,
    /// Each row's cells as text; none is NULL.
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows came back than the limit, and only the first ones are kept.
    pub truncated: bool,
    /// Rows a change touched; none for statements that return rows.
    pub affected: Option<u64>,
    /// One line about it: how many rows it returned or changed, and whether some were left out.
    pub summary: String,
    /// It as an agent is handed it: which database, the SQL that ran and a table of its first rows.
    pub for_agent: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultColumn {
    pub name: String,
    /// The engine's own name for the type; empty when it does not say.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Numbers sit to the right in a grid.
    pub numeric: bool,
}

/// How a result is copied: tab-separated, as spreadsheets paste it, or comma-separated.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CopyFormat {
    Tsv,
    Csv,
}

/// The connection form: the connection as it starts and the choices it offers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileForm {
    pub draft: ProfileDraft,
    /// A password is saved for it, so the form says so rather than showing one.
    pub has_password: bool,
    pub engines: Vec<EngineChoice>,
    pub tls_modes: Vec<TlsChoice>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineChoice {
    pub engine: Engine,
    pub label: String,
    /// The port it is reached on when none is typed; none for a file.
    pub default_port: Option<u16>,
    /// What the empty fields suggest for it.
    pub name_hint: String,
    pub user_hint: String,
    pub database_hint: String,
    pub path_hint: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TlsChoice {
    pub value: String,
    pub label: String,
    pub detail: String,
}

/// A connection as the form edits it. The fields its engine does not use are kept, so
/// switching back finds them as they were.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDraft {
    /// None until it is first saved.
    pub id: Option<String>,
    pub name: String,
    pub engine: Engine,
    pub host: String,
    /// As typed; empty for the engine's usual port.
    pub port: String,
    pub database: String,
    pub user: String,
    pub tls: String,
    pub path: String,
    pub read_only: bool,
    /// Agents may run statements that change data; never while it is read only.
    pub agent_writes: bool,
}
