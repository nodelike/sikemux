// What the Database pane shows: every saved database with what the explorer has open in it,
// one table's structure, and the queries run on a database. The core keeps each and reads it
// again on the pace the manifest gives.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::future::join_all;
use serde::Deserialize;
use sikemux_plugin_database_views::{
    Connection, Engine, Explorer, Fact, ForeignKeyRow, History, HistoryEntry, IndexRow, Schema,
    TableEntry, TableGroup, TableKind, TableView,
};

use crate::connections::{Access, Pool};
use crate::engines::{mysql::MYSQL_PORT, Session};
use crate::error::DatabaseResult;
use crate::history::{self, HistoryRequest, Source};
use crate::profiles::{self, Profile, Target, POSTGRES_PORT};
use crate::schema::{Table, TableInfo, TableRequest};

/// What the explorer has open: in each connection, its schemas and the tables whose columns show.
#[derive(Deserialize, Default)]
pub struct ExplorerAsk {
    #[serde(default)]
    pub open: Vec<Opened>,
}

#[derive(Deserialize)]
pub struct Opened {
    pub id: String,
    #[serde(default)]
    pub schemas: Vec<String>,
    #[serde(default)]
    pub tables: Vec<TableAt>,
}

#[derive(Deserialize)]
pub struct TableAt {
    pub schema: String,
    pub name: String,
}

/// Every saved database. Only one the person connected to is read, so the explorer never
/// signs in on its own.
pub async fn explorer_view(
    pool: &Pool,
    data_dir: PathBuf,
    ask: ExplorerAsk,
) -> DatabaseResult<Explorer> {
    let saved = profiles::blocking({
        let data_dir = data_dir.clone();
        move || profiles::load(&data_dir)
    })
    .await?
    .profiles;
    let connected: HashMap<String, String> = pool
        .connected()
        .await
        .into_iter()
        .map(|each| (each.id, each.version))
        .collect();
    let connections = saved
        .iter()
        .map(|profile| connection(pool, &data_dir, profile, &connected, &ask.open));
    Ok(Explorer {
        connections: join_all(connections).await,
    })
}

async fn connection(
    pool: &Pool,
    data_dir: &Path,
    profile: &Profile,
    connected: &HashMap<String, String>,
    open: &[Opened],
) -> Connection {
    let version = connected.get(&profile.id).cloned();
    let opened = open.iter().find(|opened| opened.id == profile.id);
    let (mut schemas, mut databases) = (None, None);
    let read = match (&version, opened) {
        (None, _) => Ok(()),
        (Some(_), _) if whole_server(profile) => {
            databases_on(pool, data_dir, profile, connected, open)
                .await
                .map(|each| databases = Some(each))
        }
        (Some(_), Some(opened)) => schemas_of(pool, data_dir, profile, opened)
            .await
            .map(|each| schemas = Some(each)),
        (Some(_), None) => Ok(()),
    };
    let failed = read.err().map(|error| error.to_string());
    Connection {
        id: profile.id.clone(),
        name: profile.name.clone(),
        engine: engine(&profile.target),
        engine_name: engine_name(&profile.target).into(),
        address: address(&profile.target),
        read_only: profile.read_only,
        facts: facts(profile, version.as_deref()),
        connected: version,
        default_schema: default_schema(&profile.target),
        schemas,
        databases,
        failed,
    }
}

/// A PostgreSQL server saved without a database, which the explorer shows every database of.
fn whole_server(profile: &Profile) -> bool {
    matches!(&profile.target, Target::Postgres(server) if server.database.is_empty())
}

/// Each database on the server, as a connection of its own.
async fn databases_on(
    pool: &Pool,
    data_dir: &Path,
    profile: &Profile,
    connected: &HashMap<String, String>,
    open: &[Opened],
) -> DatabaseResult<Vec<Connection>> {
    let session = pool.session(data_dir, &profile.id, Access::Browse).await?;
    let saved = profiles::blocking({
        let data_dir = data_dir.to_path_buf();
        move || profiles::load(&data_dir)
    })
    .await?;
    let mut each = Vec::new();
    for name in session.databases().await? {
        let mut database = saved.open(&format!("{}/{name}", profile.id))?;
        database.name = name;
        each.push(Box::pin(connection(pool, data_dir, &database, connected, open)).await);
    }
    Ok(each)
}

async fn schemas_of(
    pool: &Pool,
    data_dir: &Path,
    profile: &Profile,
    opened: &Opened,
) -> DatabaseResult<Vec<Schema>> {
    let session = pool.session(data_dir, &profile.id, Access::Browse).await?;
    let mut schemas = Vec::new();
    for name in session.schemas().await? {
        let (groups, failed) = match opened.schemas.contains(&name) {
            false => (None, None),
            true => match groups(&session, &profile.target, &name, opened).await {
                Ok(groups) => (Some(groups), None),
                Err(error) => (None, Some(error.to_string())),
            },
        };
        schemas.push(Schema {
            name,
            groups,
            failed,
        });
    }
    Ok(schemas)
}

/// The schema's tables, then its views, each with its columns when the explorer shows them.
async fn groups(
    session: &Session,
    target: &Target,
    schema: &str,
    opened: &Opened,
) -> DatabaseResult<Vec<TableGroup>> {
    let mut tables = Vec::new();
    let mut views = Vec::new();
    for table in session.tables(Some(schema.to_owned())).await? {
        let open = opened
            .tables
            .iter()
            .any(|at| at.schema == schema && at.name == table.name);
        let (columns, failed) = match open {
            false => (None, None),
            true => match session
                .describe(Some(schema.to_owned()), table.name.clone())
                .await
            {
                Ok(info) => (Some(info.columns), None),
                Err(error) => (None, Some(error.to_string())),
            },
        };
        let entry = entry(target, schema, table, columns, failed);
        match entry.kind {
            TableKind::Table | TableKind::ForeignTable => tables.push(entry),
            TableKind::View | TableKind::MaterializedView => views.push(entry),
        }
    }
    Ok([("tables", tables), ("views", views)]
        .into_iter()
        .filter(|(_, tables)| !tables.is_empty())
        .map(|(name, tables)| TableGroup {
            name: name.into(),
            tables,
        })
        .collect())
}

fn entry(
    target: &Target,
    schema: &str,
    table: Table,
    columns: Option<Vec<sikemux_plugin_database_views::Column>>,
    failed: Option<String>,
) -> TableEntry {
    TableEntry {
        preview: preview(target, schema, &table.name),
        kind_name: kind_name(table.kind).into(),
        name: table.name,
        kind: table.kind,
        columns,
        failed,
    }
}

pub async fn table_view(
    pool: &Pool,
    data_dir: PathBuf,
    ask: TableRequest,
) -> DatabaseResult<TableView> {
    let saved = profiles::blocking({
        let data_dir = data_dir.clone();
        let id = ask.id.clone();
        move || profiles::load(&data_dir)?.open(&id)
    })
    .await?;
    let info = pool
        .session(&data_dir, &ask.id, Access::Browse)
        .await?
        .describe(ask.schema, ask.table)
        .await?;
    Ok(structure(&saved.target, info))
}

fn structure(target: &Target, info: TableInfo) -> TableView {
    let indexes = info
        .indexes
        .into_iter()
        .map(|index| {
            let what = match (index.primary, index.unique) {
                (true, _) => " primary",
                (false, true) => " unique",
                (false, false) => "",
            };
            IndexRow {
                detail: format!("({}){what}", index.columns.join(", ")),
                name: index.name,
            }
        })
        .collect();
    let foreign_keys = info
        .foreign_keys
        .into_iter()
        .map(|key| ForeignKeyRow {
            columns: key.columns.join(", "),
            schema: key.references_schema.unwrap_or_else(|| info.schema.clone()),
            target: format!(
                "{}({})",
                key.references_table,
                key.references_columns.join(", ")
            ),
            table: key.references_table,
        })
        .collect();
    TableView {
        preview: preview(target, &info.schema, &info.name),
        kind_name: kind_name(info.kind).into(),
        schema: info.schema,
        name: info.name,
        kind: info.kind,
        columns: info.columns,
        indexes,
        foreign_keys,
    }
}

pub async fn history_view(data_dir: PathBuf, ask: HistoryRequest) -> DatabaseResult<History> {
    let searched = ask
        .search
        .as_deref()
        .is_some_and(|search| !search.trim().is_empty());
    let entries = profiles::blocking(move || history::list(&data_dir, &ask)).await?;
    Ok(History {
        entries: entries
            .into_iter()
            .map(|entry| HistoryEntry {
                by_agent: entry.source == Source::Agent,
                sql: entry.sql,
                at: entry.at,
                millis: entry.millis,
                ok: entry.ok,
                rows: entry.rows,
                error: entry.error,
            })
            .collect(),
        empty: match searched {
            true => "No past query matches.",
            false => "Queries you and your agents run here show up in this list.",
        }
        .into(),
    })
}

fn engine(target: &Target) -> Engine {
    match target {
        Target::Postgres(_) => Engine::Postgres,
        Target::Mysql(_) => Engine::Mysql,
        Target::Sqlite { .. } => Engine::Sqlite,
    }
}

pub(crate) fn engine_name(target: &Target) -> &'static str {
    match target {
        Target::Postgres(_) => "PostgreSQL",
        Target::Mysql(_) => "MySQL",
        Target::Sqlite { .. } => "SQLite",
    }
}

fn default_port(target: &Target) -> u16 {
    match target {
        Target::Mysql(_) => MYSQL_PORT,
        _ => POSTGRES_PORT,
    }
}

/// Where the database is: `app@localhost:5432/shop`, or the file's name.
fn address(target: &Target) -> String {
    match target {
        Target::Postgres(server) | Target::Mysql(server) => {
            let database = match server.database.is_empty() {
                true => String::new(),
                false => format!("/{}", server.database),
            };
            let port = server.port.unwrap_or(default_port(target));
            format!("{}@{}:{port}{database}", server.user, server.host)
        }
        Target::Sqlite { path } => Path::new(path)
            .file_name()
            .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned()),
    }
}

fn default_schema(target: &Target) -> String {
    match target {
        Target::Postgres(server) if server.database.is_empty() => String::new(),
        Target::Postgres(_) => "public".into(),
        Target::Mysql(server) => server.database.clone(),
        Target::Sqlite { .. } => "main".into(),
    }
}

fn facts(profile: &Profile, connected: Option<&str>) -> Vec<Fact> {
    let fact = |label: &str, value: String, mono: bool| Fact {
        label: label.into(),
        value,
        mono,
    };
    let mut facts = vec![fact("Engine", engine_name(&profile.target).into(), false)];
    match &profile.target {
        Target::Postgres(server) | Target::Mysql(server) => {
            let port = server.port.unwrap_or(default_port(&profile.target));
            let database = match server.database.is_empty() {
                true => "All databases".into(),
                false => server.database.clone(),
            };
            let password = match profile.has_password {
                true => "Saved in the Keychain",
                false => "None",
            };
            facts.extend([
                fact("Server", format!("{}:{port}", server.host), true),
                fact("Database", database, true),
                fact("User", server.user.clone(), true),
                fact("Password", password.into(), false),
            ]);
        }
        Target::Sqlite { path } => facts.push(fact("File", path.clone(), true)),
    }
    let agents = match profile.agent_writes {
        true => "May change data",
        false => "Read only",
    };
    let status = match connected {
        Some(version) => format!("Connected to {version}"),
        None => "Not connected".into(),
    };
    facts.extend([
        fact("Agents", agents.into(), false),
        fact("Status", status, false),
    ]);
    facts
}

fn kind_name(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Table => "table",
        TableKind::View => "view",
        TableKind::MaterializedView => "materialized view",
        TableKind::ForeignTable => "foreign table",
    }
}

/// A name written so the engine reads it as one identifier: backticks for MySQL, double
/// quotes elsewhere.
fn quoted(target: &Target, name: &str) -> String {
    match target {
        Target::Mysql(_) => format!("`{}`", name.replace('`', "``")),
        _ => crate::schema::quote_identifier(name),
    }
}

/// A query for a table's first rows. A SQLite table in its main schema goes unqualified.
fn preview(target: &Target, schema: &str, table: &str) -> String {
    let qualified = match (target, schema) {
        (Target::Sqlite { .. }, "main") | (_, "") => quoted(target, table),
        _ => format!("{}.{}", quoted(target, schema), quoted(target, table)),
    };
    format!("select * from {qualified} limit 100;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{Server, Tls};

    fn shop() -> Server {
        Server {
            host: "db.internal".into(),
            port: None,
            database: "shop".into(),
            user: "app".into(),
            tls: Tls::Prefer,
        }
    }

    #[test]
    fn says_where_each_database_is_and_the_schema_it_opens_on() {
        let sqlite = Target::Sqlite {
            path: "/Users/me/data/app.db".into(),
        };
        let mysql = Target::Mysql(Server {
            database: "app".into(),
            ..shop()
        });
        let unnamed = Target::Postgres(Server {
            port: Some(6543),
            database: String::new(),
            ..shop()
        });
        assert_eq!(
            address(&Target::Postgres(shop())),
            "app@db.internal:5432/shop"
        );
        assert_eq!(address(&unnamed), "app@db.internal:6543");
        assert_eq!(address(&sqlite), "app.db");
        assert_eq!(address(&mysql), "app@db.internal:3306/app");
        assert_eq!(default_schema(&Target::Postgres(shop())), "public");
        assert_eq!(default_schema(&mysql), "app");
        assert_eq!(default_schema(&sqlite), "main");
    }

    #[test]
    fn previews_a_table_with_names_quoted_as_each_engine_reads_them() {
        let postgres = Target::Postgres(shop());
        let mysql = Target::Mysql(shop());
        let sqlite = Target::Sqlite {
            path: "/a.db".into(),
        };
        assert_eq!(
            preview(&postgres, "public", "orders"),
            r#"select * from "public"."orders" limit 100;"#
        );
        assert_eq!(
            preview(&postgres, "public", r#"odd "name""#),
            r#"select * from "public"."odd ""name""" limit 100;"#
        );
        assert_eq!(
            preview(&mysql, "shop", "odd `name`"),
            "select * from `shop`.`odd ``name``` limit 100;"
        );
        assert_eq!(
            preview(&sqlite, "main", "orders"),
            r#"select * from "orders" limit 100;"#
        );
    }
}
