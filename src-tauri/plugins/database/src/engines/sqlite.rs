// SQLite: a file on this Mac, opened in place. Every call runs on a thread
// meant for blocking, since SQLite does its work on the calling thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::types::ValueRef;
use rusqlite::{Batch, Connection, InterruptHandle, OpenFlags};

use crate::error::{DatabaseError, DatabaseResult};
use crate::schema::{quote_identifier, ColumnInfo, ForeignKey, Index, Table, TableInfo, TableKind};
use crate::values::{self, Column, ResultSet};

#[derive(Clone)]
pub struct Session {
    connection: Arc<Mutex<Connection>>,
    /// Reaches a running statement from another thread, since the connection itself stays locked while it runs.
    interrupt: Arc<InterruptHandle>,
    alive: Arc<AtomicBool>,
}

/// `~/data/app.db` is the person's own home folder, as it would be in a terminal.
fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

impl Session {
    /// Opens the file without ever creating it, so a mistyped path is an error rather than an empty database.
    pub async fn open(path: &str, read_only: bool) -> DatabaseResult<Self> {
        let file = expand(path);
        tokio::task::spawn_blocking(move || {
            if !file.is_file() {
                return Err(DatabaseError::Connect(format!(
                    "there is no database file at {}",
                    file.display()
                )));
            }
            let access = if read_only {
                OpenFlags::SQLITE_OPEN_READ_ONLY
            } else {
                OpenFlags::SQLITE_OPEN_READ_WRITE
            };
            let connection =
                Connection::open_with_flags(&file, access | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                    .map_err(|error| DatabaseError::Connect(format!("sqlite: {error}")))?;
            connection
                .busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|error| DatabaseError::Connect(format!("sqlite: {error}")))?;
            Ok(Self {
                interrupt: Arc::new(connection.get_interrupt_handle()),
                connection: Arc::new(Mutex::new(connection)),
                alive: Arc::new(AtomicBool::new(true)),
            })
        })
        .await
        .map_err(|error| DatabaseError::Connect(error.to_string()))?
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn retire(&self) {
        self.alive.store(false, Ordering::Relaxed);
    }

    /// Runs work against the connection on a thread meant for blocking.
    pub async fn with<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> DatabaseResult<T> {
        let connection = Arc::clone(&self.connection);
        tokio::task::spawn_blocking(move || {
            let connection = connection
                .lock()
                .map_err(|_| DatabaseError::Query("sqlite: an earlier query crashed".into()))?;
            work(&connection).map_err(|error| DatabaseError::Query(format!("sqlite: {error}")))
        })
        .await
        .map_err(|error| DatabaseError::Query(error.to_string()))?
    }

    pub async fn version(&self) -> DatabaseResult<String> {
        let version = self
            .with(|connection| {
                connection.query_row("select sqlite_version()", [], |row| row.get::<_, String>(0))
            })
            .await?;
        Ok(format!("SQLite {version}"))
    }

    /// `main`, and any database attached to it.
    pub async fn schemas(&self) -> DatabaseResult<Vec<String>> {
        self.with(|connection| {
            let mut statement = connection.prepare(
                "select name from pragma_database_list where name != 'temp' order by seq",
            )?;
            let names = statement.query_map([], |row| row.get::<_, String>(0))?;
            names.collect()
        })
        .await
    }

    pub async fn tables(&self, schema: String) -> DatabaseResult<Vec<Table>> {
        self.with(move |connection| {
            let sql = format!(
                "select name, type from {}.sqlite_master \
                 where type in ('table', 'view') and name not like 'sqlite\\_%' escape '\\' order by name",
                quote_identifier(&schema)
            );
            let mut statement = connection.prepare(&sql)?;
            let tables = statement.query_map([], |row| {
                let kind: String = row.get(1)?;
                Ok(Table {
                    name: row.get(0)?,
                    kind: if kind == "view" { TableKind::View } else { TableKind::Table },
                })
            })?;
            tables.collect()
        })
        .await
    }

    /// Stops whatever statement is running now; it ends with an "interrupted" error.
    pub fn cancel(&self) {
        self.interrupt.interrupt();
    }

    pub async fn query(&self, sql: String, limit: usize) -> DatabaseResult<Vec<ResultSet>> {
        self.with(move |connection| run(connection, &sql, limit))
            .await
    }

    pub async fn describe(&self, schema: String, table: String) -> DatabaseResult<TableInfo> {
        let wanted = format!("{schema}.{table}");
        let found = self
            .with(move |connection| describe(connection, &schema, &table))
            .await?;
        found.ok_or_else(|| {
            DatabaseError::NotFound(format!("there is no table or view named {wanted}"))
        })
    }
}

fn cell(value: ValueRef<'_>) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(number) => values::integer(number),
        ValueRef::Real(number) => values::real(number),
        ValueRef::Text(bytes) => values::text(&String::from_utf8_lossy(bytes)),
        ValueRef::Blob(bytes) => values::blob(bytes),
    }
}

/// Runs every statement in the text, one after another, keeping at most `limit` rows from each.
fn run(connection: &Connection, sql: &str, limit: usize) -> rusqlite::Result<Vec<ResultSet>> {
    let mut batch = Batch::new(connection, sql);
    let mut results = Vec::new();
    while let Some(mut statement) = batch.next()? {
        let mut columns: Vec<Column> = statement
            .columns()
            .iter()
            .map(|column| {
                let type_name = column.decl_type().unwrap_or_default().to_string();
                Column {
                    name: column.name().to_string(),
                    numeric: values::is_numeric_type(&type_name),
                    type_name,
                }
            })
            .collect();
        if columns.is_empty() {
            let affected = statement.execute([])?;
            results.push(ResultSet {
                affected: u64::try_from(affected).ok(),
                ..ResultSet::default()
            });
            continue;
        }
        let width = columns.len();
        let mut rows = Vec::new();
        let mut truncated = false;
        let mut cursor = statement.query([])?;
        while let Some(row) = cursor.next()? {
            if rows.len() == limit {
                truncated = true;
                break;
            }
            rows.push(
                (0..width)
                    .map(|index| row.get_ref(index).map(cell))
                    .collect::<rusqlite::Result<Vec<_>>>()?,
            );
        }
        values::infer_numeric(&mut columns, &rows);
        results.push(ResultSet {
            columns,
            rows,
            truncated,
            affected: None,
        });
    }
    Ok(results)
}

fn describe(
    connection: &Connection,
    schema: &str,
    table: &str,
) -> rusqlite::Result<Option<TableInfo>> {
    let kind_sql = format!(
        "select type from {}.sqlite_master where name = ?1 and type in ('table', 'view')",
        quote_identifier(schema)
    );
    let kind = match connection.query_row(&kind_sql, [table], |row| row.get::<_, String>(0)) {
        Ok(kind) => kind,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut statement =
        connection.prepare("select name, type, \"notnull\", dflt_value, pk from pragma_table_info(?1, ?2) order by cid")?;
    let columns = statement
        .query_map([table, schema], |row| {
            Ok(ColumnInfo {
                name: row.get(0)?,
                type_name: row.get(1)?,
                nullable: row.get::<_, i64>(2)? == 0,
                default: row.get(3)?,
                primary_key: row.get::<_, i64>(4)? > 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = connection
        .prepare("select name, \"unique\", origin from pragma_index_list(?1, ?2) order by name")?;
    let listed = statement
        .query_map([table, schema], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? == 1,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement =
        connection.prepare("select name from pragma_index_info(?1, ?2) order by seqno")?;
    let mut indexes = Vec::with_capacity(listed.len());
    for (name, unique, origin) in listed {
        let index_columns = statement
            .query_map([name.as_str(), schema], |row| {
                row.get::<_, Option<String>>(0)
            })?
            .map(|column| column.map(Option::unwrap_or_default))
            .collect::<rusqlite::Result<Vec<_>>>()?;
        indexes.push(Index {
            name,
            columns: index_columns,
            unique,
            primary: origin == "pk",
        });
    }
    let mut statement = connection
        .prepare("select id, \"table\", \"from\", \"to\" from pragma_foreign_key_list(?1, ?2) order by id, seq")?;
    let mut foreign_keys: Vec<(i64, ForeignKey)> = Vec::new();
    let rows = statement.query_map([table, schema], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (id, references_table, from, to) = row?;
        match foreign_keys.last_mut() {
            Some((last, key)) if *last == id => {
                key.columns.push(from);
                key.references_columns.extend(to);
            }
            _ => foreign_keys.push((
                id,
                ForeignKey {
                    name: None,
                    columns: vec![from],
                    references_schema: None,
                    references_table,
                    references_columns: to.into_iter().collect(),
                },
            )),
        }
    }
    Ok(Some(TableInfo {
        schema: schema.to_string(),
        name: table.to_string(),
        kind: if kind == "view" {
            TableKind::View
        } else {
            TableKind::Table
        },
        columns,
        indexes,
        foreign_keys: foreign_keys.into_iter().map(|(_, key)| key).collect(),
    }))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A fresh database file with a small table in it.
    pub fn fixture(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("sikemux-database-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "create table customers (id integer primary key, name text not null, email text unique);
                 insert into customers (name, email) values ('Ada', 'ada@example.com'), ('Linus', null);
                 create table orders (id integer primary key, customer_id integer not null references customers(id),
                     total real default 0, note blob);
                 create index orders_by_customer on orders (customer_id);
                 insert into orders (customer_id, total, note) values (1, 42.5, x'cafe'), (1, 9.99, null);
                 create view big_orders as select * from orders where total > 10;",
            )
            .unwrap();
        path
    }

    #[tokio::test]
    async fn it_opens_a_file_and_reads_its_version() {
        let path = fixture("open");
        let session = Session::open(path.to_str().unwrap(), false).await.unwrap();
        assert!(session.version().await.unwrap().starts_with("SQLite 3."));
        let count = session
            .with(|connection| {
                connection.query_row("select count(*) from customers", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn a_missing_file_is_never_created() {
        let path = std::env::temp_dir().join(format!(
            "sikemux-database-missing-{}.db",
            std::process::id()
        ));
        let opened = Session::open(path.to_str().unwrap(), false).await;
        assert!(matches!(opened, Err(DatabaseError::Connect(_))));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn read_only_refuses_writes() {
        let path = fixture("read-only");
        let session = Session::open(path.to_str().unwrap(), true).await.unwrap();
        let write = session
            .with(|connection| connection.execute("delete from customers", []))
            .await;
        assert!(matches!(write, Err(DatabaseError::Query(_))));
    }

    #[tokio::test]
    async fn it_lists_schemas_and_tables_and_views() {
        let path = fixture("tables");
        let session = Session::open(path.to_str().unwrap(), true).await.unwrap();
        assert_eq!(session.schemas().await.unwrap(), vec!["main".to_string()]);
        let tables = session.tables("main".into()).await.unwrap();
        let named: Vec<(&str, TableKind)> = tables
            .iter()
            .map(|table| (table.name.as_str(), table.kind))
            .collect();
        assert_eq!(
            named,
            vec![
                ("big_orders", TableKind::View),
                ("customers", TableKind::Table),
                ("orders", TableKind::Table)
            ]
        );
    }

    #[tokio::test]
    async fn it_describes_a_table_with_its_keys_and_indexes() {
        let path = fixture("describe");
        let session = Session::open(path.to_str().unwrap(), true).await.unwrap();
        let orders = session
            .describe("main".into(), "orders".into())
            .await
            .unwrap();
        let columns: Vec<(&str, &str, bool, bool)> = orders
            .columns
            .iter()
            .map(|column| {
                (
                    column.name.as_str(),
                    column.type_name.as_str(),
                    column.nullable,
                    column.primary_key,
                )
            })
            .collect();
        assert_eq!(
            columns,
            vec![
                ("id", "INTEGER", true, true),
                ("customer_id", "INTEGER", false, false),
                ("total", "REAL", true, false),
                ("note", "BLOB", true, false)
            ]
        );
        assert_eq!(orders.columns[2].default.as_deref(), Some("0"));
        assert_eq!(orders.indexes.len(), 1);
        assert_eq!(orders.indexes[0].columns, vec!["customer_id".to_string()]);
        assert_eq!(orders.foreign_keys.len(), 1);
        assert_eq!(orders.foreign_keys[0].references_table, "customers");
        assert_eq!(
            orders.foreign_keys[0].references_columns,
            vec!["id".to_string()]
        );

        let customers = session
            .describe("main".into(), "customers".into())
            .await
            .unwrap();
        assert!(customers
            .indexes
            .iter()
            .any(|index| index.unique && index.columns == ["email"]));
        let view = session
            .describe("main".into(), "big_orders".into())
            .await
            .unwrap();
        assert_eq!(view.kind, TableKind::View);
        assert!(matches!(
            session.describe("main".into(), "nope".into()).await,
            Err(DatabaseError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn a_query_returns_typed_cells_and_marks_number_columns() {
        let path = fixture("query");
        let session = Session::open(path.to_str().unwrap(), false).await.unwrap();
        let results = session
            .query("select id, name, email, (select count(*) from orders) as n from customers order by id".into(), 100)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        let result = &results[0];
        let names: Vec<&str> = result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect();
        assert_eq!(names, vec!["id", "name", "email", "n"]);
        let numeric: Vec<bool> = result.columns.iter().map(|column| column.numeric).collect();
        assert_eq!(numeric, vec![true, false, false, true]);
        assert_eq!(
            result.rows[0],
            vec![
                serde_json::json!(1),
                "Ada".into(),
                "ada@example.com".into(),
                serde_json::json!(2)
            ]
        );
        assert_eq!(result.rows[1][2], serde_json::Value::Null);
        let blob = session
            .query("select note from orders where id = 1".into(), 10)
            .await
            .unwrap();
        assert_eq!(
            blob[0].rows[0][0],
            serde_json::Value::String("\\xcafe".into())
        );
    }

    #[tokio::test]
    async fn every_statement_runs_and_changes_say_how_many_rows_they_touched() {
        let path = fixture("script");
        let session = Session::open(path.to_str().unwrap(), false).await.unwrap();
        let results = session
            .query(
                "update orders set total = total + 1; insert into customers (name) values ('Grace'); select count(*) from customers;"
                    .into(),
                10,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].affected, Some(2));
        assert_eq!(results[1].affected, Some(1));
        assert_eq!(results[2].rows, vec![vec![serde_json::json!(3)]]);
    }

    #[tokio::test]
    async fn rows_past_the_limit_are_left_out_and_flagged() {
        let path = fixture("limit");
        let session = Session::open(path.to_str().unwrap(), false).await.unwrap();
        let results = session
            .query("select * from customers".into(), 1)
            .await
            .unwrap();
        assert_eq!(results[0].rows.len(), 1);
        assert!(results[0].truncated);
        let error = session.query("select * from nowhere".into(), 1).await;
        let Err(DatabaseError::Query(message)) = error else {
            panic!("expected a query error")
        };
        assert!(message.contains("no such table"), "{message}");
    }

    #[tokio::test]
    async fn a_running_query_can_be_cancelled() {
        let path = fixture("cancel");
        let session = Session::open(path.to_str().unwrap(), true).await.unwrap();
        let running = session.clone();
        let endless = tokio::spawn(async move {
            running
                .query(
                    "with recursive n(i) as (select 1 union all select i + 1 from n) select count(*) from n".into(),
                    1,
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        session.cancel();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), endless)
            .await
            .unwrap()
            .unwrap();
        let Err(DatabaseError::Query(message)) = outcome else {
            panic!("expected the query to be interrupted")
        };
        assert!(message.contains("interrupt"), "{message}");
        assert!(session.query("select 1".into(), 1).await.is_ok());
    }

    #[test]
    fn a_tilde_is_the_home_folder() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand("~/a.db"), PathBuf::from(home).join("a.db"));
        assert_eq!(expand("/tmp/a.db"), PathBuf::from("/tmp/a.db"));
    }
}
