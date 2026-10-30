// MySQL and MariaDB over the network, with one connection kept open between
// calls. Results come over the text protocol, so several statements can run
// at once, and each value is read back by its column's type.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{
    Column as MysqlColumn, Conn, Opts, OptsBuilder, Row, SslOpts, Value as MysqlValue,
};

use super::Unfinished;
use crate::error::{DatabaseError, DatabaseResult};
use crate::profiles::{Server, Tls};
use crate::schema::{ColumnInfo, ForeignKey, Index, Table, TableInfo, TableKind};
use crate::values::{self, Column, ResultSet};

pub const MYSQL_PORT: u16 = 3306;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The character set MySQL gives binary strings and blobs.
const BINARY_CHARSET: u16 = 63;
/// A guard against a server that keeps announcing more results.
const MAX_RESULT_SETS: usize = 1_000;

#[derive(Clone)]
pub struct Session {
    connection: Arc<tokio::sync::Mutex<Conn>>,
    /// How to reach the server again, to ask it to stop a query.
    opts: Opts,
    connection_id: u32,
    /// The database named in the profile, where tables are looked for when no schema is named.
    database: String,
    alive: Arc<AtomicBool>,
}

/// MySQL's own words for a failure, without the driver's wrapping.
pub fn describe(error: &mysql_async::Error) -> String {
    match error {
        mysql_async::Error::Server(server) => server.message.clone(),
        mysql_async::Error::Other(other) => other.to_string(),
        other => other.to_string(),
    }
}

fn ssl_opts(tls: Tls) -> Option<SslOpts> {
    match tls {
        Tls::Disable => None,
        Tls::Prefer | Tls::Require => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        Tls::VerifyFull => Some(SslOpts::default()),
    }
}

fn options(server: &Server, password: Option<&str>, read_only: bool, tls: Option<SslOpts>) -> Opts {
    let database = Some(server.database.clone()).filter(|database| !database.is_empty());
    let setup = if read_only {
        vec!["set session transaction read only"]
    } else {
        Vec::new()
    };
    OptsBuilder::default()
        .ip_or_hostname(server.host.clone())
        .tcp_port(server.port.unwrap_or(MYSQL_PORT))
        .user(Some(server.user.clone()))
        .pass(password.filter(|password| !password.is_empty()))
        .db_name(database)
        .prefer_socket(false)
        .ssl_opts(tls)
        .init(setup)
        .into()
}

async fn connect(opts: Opts) -> Result<Conn, mysql_async::Error> {
    let host = opts.ip_or_hostname().to_string();
    let port = opts.tcp_port();
    match tokio::time::timeout(CONNECT_TIMEOUT, Conn::new(opts)).await {
        Ok(connected) => connected,
        Err(_) => Err(mysql_async::Error::Other(
            format!(
                "{host}:{port} did not answer within {}s",
                CONNECT_TIMEOUT.as_secs()
            )
            .into(),
        )),
    }
}

fn connect_error(error: &mysql_async::Error) -> DatabaseError {
    DatabaseError::Connect(describe(error))
}

/// The server answered but cannot encrypt, which is when `prefer` goes on without it.
fn lacks_tls(error: &mysql_async::Error) -> bool {
    matches!(
        error,
        mysql_async::Error::Driver(mysql_async::DriverError::NoClientSslFlagFromServer)
    )
}

/// One row past the limit, so a result that was cut still says so.
fn select_limit(limit: usize) -> String {
    format!("set sql_select_limit = {}", limit.saturating_add(1))
}

fn is_numeric(ty: ColumnType) -> bool {
    use ColumnType::*;
    matches!(
        ty,
        MYSQL_TYPE_TINY
            | MYSQL_TYPE_SHORT
            | MYSQL_TYPE_INT24
            | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_FLOAT
            | MYSQL_TYPE_DOUBLE
            | MYSQL_TYPE_DECIMAL
            | MYSQL_TYPE_NEWDECIMAL
            | MYSQL_TYPE_YEAR
    )
}

/// `MYSQL_TYPE_LONGLONG` as people know it: `longlong`.
fn type_name(ty: ColumnType) -> String {
    let name = format!("{ty:?}");
    name.strip_prefix("MYSQL_TYPE_")
        .unwrap_or(&name)
        .to_ascii_lowercase()
}

fn is_binary(column: &MysqlColumn) -> bool {
    use ColumnType::*;
    column.character_set() == BINARY_CHARSET
        && matches!(
            column.column_type(),
            MYSQL_TYPE_BLOB
                | MYSQL_TYPE_TINY_BLOB
                | MYSQL_TYPE_MEDIUM_BLOB
                | MYSQL_TYPE_LONG_BLOB
                | MYSQL_TYPE_STRING
                | MYSQL_TYPE_VAR_STRING
                | MYSQL_TYPE_VARCHAR
                | MYSQL_TYPE_BIT
                | MYSQL_TYPE_GEOMETRY
        )
}

/// A value as MySQL sends it in text, turned back into what its column holds.
/// Decimals stay text so no digit is lost.
fn cell(value: Option<&MysqlValue>, column: &MysqlColumn) -> serde_json::Value {
    use ColumnType::*;
    let bytes = match value {
        None | Some(MysqlValue::NULL) => return serde_json::Value::Null,
        Some(MysqlValue::Bytes(bytes)) => bytes,
        Some(MysqlValue::Int(number)) => return values::integer(*number),
        Some(MysqlValue::UInt(number)) => {
            return i64::try_from(*number)
                .map_or_else(|_| values::text(&number.to_string()), values::integer)
        }
        Some(MysqlValue::Float(number)) => return values::real(f64::from(*number)),
        Some(MysqlValue::Double(number)) => return values::real(*number),
        Some(other) => return values::text(&other.as_sql(true)),
    };
    if is_binary(column) {
        return values::blob(bytes);
    }
    let text = String::from_utf8_lossy(bytes);
    match column.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG | MYSQL_TYPE_YEAR => text
            .parse()
            .map_or_else(|_| values::text(&text), values::integer),
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => text
            .parse()
            .map_or_else(|_| values::text(&text), values::real),
        _ => values::text(&text),
    }
}

fn result_columns(columns: &[MysqlColumn]) -> Vec<Column> {
    columns
        .iter()
        .map(|column| Column {
            name: column.name_str().into_owned(),
            type_name: type_name(column.column_type()),
            numeric: is_numeric(column.column_type()),
        })
        .collect()
}

fn row_cells(row: &Row, columns: &[MysqlColumn]) -> Vec<serde_json::Value> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| cell(row.as_ref(index), column))
        .collect()
}

impl Session {
    /// With `prefer`, a server that will not encrypt is reached without it, as MySQL's own client does.
    pub async fn open(
        server: &Server,
        password: Option<&str>,
        read_only: bool,
    ) -> DatabaseResult<Self> {
        let encrypted = options(server, password, read_only, ssl_opts(server.tls));
        let (connection, opts) = match connect(encrypted.clone()).await {
            Ok(connection) => (connection, encrypted),
            Err(error) if server.tls == Tls::Prefer && lacks_tls(&error) => {
                let plain = options(server, password, read_only, None);
                let connection = connect(plain.clone())
                    .await
                    .map_err(|error| connect_error(&error))?;
                (connection, plain)
            }
            Err(error) => return Err(connect_error(&error)),
        };
        Ok(Self {
            connection_id: connection.id(),
            connection: Arc::new(tokio::sync::Mutex::new(connection)),
            opts,
            database: server.database.clone(),
            alive: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn retire(&self) {
        self.alive.store(false, Ordering::Relaxed);
    }

    /// The database named in the profile; empty when it names none.
    pub fn default_schema(&self) -> String {
        self.database.clone()
    }

    fn failed(&self, error: &mysql_async::Error) -> DatabaseError {
        if matches!(
            error,
            mysql_async::Error::Io(_) | mysql_async::Error::Driver(_)
        ) {
            self.alive.store(false, Ordering::Relaxed);
        }
        DatabaseError::Query(describe(error))
    }

    pub async fn version(&self) -> DatabaseResult<String> {
        let mut connection = self.connection.lock().await;
        let version: Option<String> = connection
            .query_first("select version()")
            .await
            .map_err(|error| self.failed(&error))?;
        let version = version.unwrap_or_default();
        let engine = if version.to_lowercase().contains("mariadb") {
            "MariaDB"
        } else {
            "MySQL"
        };
        Ok(format!(
            "{engine} {}",
            version.split('-').next().unwrap_or(&version)
        ))
    }

    /// Runs every statement in the text, keeping at most `limit` rows from each. The server is told to send
    /// no more than one row past the limit, so a huge select is not read to its end; a select with a `limit`
    /// of its own keeps that one.
    pub async fn query(&self, sql: &str, limit: usize) -> DatabaseResult<Vec<ResultSet>> {
        let mut connection = self.connection.lock().await;
        let unfinished = Unfinished::new(&self.alive);
        connection
            .query_drop(select_limit(limit))
            .await
            .map_err(|error| self.failed(&error))?;
        let outcome = self.run(&mut connection, sql, limit).await;
        let reset = connection
            .query_drop("set sql_select_limit = default")
            .await
            .map_err(|error| self.failed(&error));
        if reset.is_ok() {
            unfinished.finish();
        }
        let results = outcome?;
        reset?;
        Ok(results)
    }

    /// For an agent on a read-only connection. A session setting could otherwise switch read-only off for the
    /// statements after it, so the SQL must be one statement, and it runs in a read-only transaction that is
    /// always rolled back. A connection the rollback did not reach is not used again, since the session's own
    /// read-only setting could be left switched off.
    pub async fn query_guarded(&self, sql: &str, limit: usize) -> DatabaseResult<Vec<ResultSet>> {
        let mut connection = self.connection.lock().await;
        let statement = connection.prep(sql).await.map_err(|error| {
            DatabaseError::Query(format!(
                "on a read-only connection an agent runs one statement at a time, which MySQL could not read here: {}",
                describe(&error)
            ))
        })?;
        connection
            .close(statement)
            .await
            .map_err(|error| self.failed(&error))?;
        let unfinished = Unfinished::new(&self.alive);
        connection
            .query_drop(format!(
                "{}; start transaction read only",
                select_limit(limit)
            ))
            .await
            .map_err(|error| self.failed(&error))?;
        let outcome = self.run(&mut connection, sql, limit).await;
        let rolled_back = connection
            .query_drop(
                "rollback; set session transaction read only; set sql_select_limit = default",
            )
            .await
            .map_err(|error| self.failed(&error));
        if rolled_back.is_ok() {
            unfinished.finish();
        }
        let results = outcome?;
        rolled_back?;
        Ok(results)
    }

    async fn run(
        &self,
        connection: &mut Conn,
        sql: &str,
        limit: usize,
    ) -> DatabaseResult<Vec<ResultSet>> {
        let mut result = connection
            .query_iter(sql)
            .await
            .map_err(|error| self.failed(&error))?;
        let mut results = Vec::new();
        while results.len() < MAX_RESULT_SETS {
            let columns = result
                .columns()
                .map(|columns| columns.to_vec())
                .unwrap_or_default();
            if columns.is_empty() {
                results.push(ResultSet {
                    affected: Some(result.affected_rows()),
                    ..ResultSet::default()
                });
                result.next().await.map_err(|error| self.failed(&error))?;
            } else {
                let mut set = ResultSet {
                    columns: result_columns(&columns),
                    ..ResultSet::default()
                };
                while let Some(row) = result.next().await.map_err(|error| self.failed(&error))? {
                    if set.rows.len() == limit {
                        set.truncated = true;
                    } else {
                        set.rows.push(row_cells(&row, &columns));
                    }
                }
                results.push(set);
            }
            if result.is_empty() {
                break;
            }
        }
        Ok(results)
    }

    /// Asks the server, over a second connection, to stop the statement this one is running.
    pub async fn cancel(&self) -> DatabaseResult<()> {
        let mut killer = connect(self.opts.clone())
            .await
            .map_err(|error| connect_error(&error))?;
        let killed = killer
            .query_drop(format!("kill query {}", self.connection_id))
            .await
            .map_err(|error| DatabaseError::Query(describe(&error)));
        let _ = killer.disconnect().await;
        killed
    }

    pub async fn schemas(&self) -> DatabaseResult<Vec<String>> {
        let mut connection = self.connection.lock().await;
        connection
            .query(
                "select schema_name from information_schema.schemata \
                 where schema_name not in ('mysql', 'information_schema', 'performance_schema', 'sys') \
                 order by schema_name",
            )
            .await
            .map_err(|error| self.failed(&error))
    }

    pub async fn tables(&self, schema: &str) -> DatabaseResult<Vec<Table>> {
        let mut connection = self.connection.lock().await;
        let rows: Vec<(String, String)> = connection
            .exec(
                "select table_name, table_type from information_schema.tables \
                 where table_schema = ? order by table_name",
                (schema,),
            )
            .await
            .map_err(|error| self.failed(&error))?;
        Ok(rows
            .into_iter()
            .map(|(name, kind)| Table {
                name,
                kind: if kind.contains("VIEW") {
                    TableKind::View
                } else {
                    TableKind::Table
                },
            })
            .collect())
    }

    pub async fn describe(&self, schema: &str, table: &str) -> DatabaseResult<TableInfo> {
        let mut connection = self.connection.lock().await;
        let kind: Option<String> = connection
            .exec_first(
                "select table_type from information_schema.tables where table_schema = ? and table_name = ?",
                (schema, table),
            )
            .await
            .map_err(|error| self.failed(&error))?;
        let kind = kind.ok_or_else(|| {
            DatabaseError::NotFound(format!("there is no table or view named {schema}.{table}"))
        })?;
        let columns: Vec<(String, String, String, Option<String>, String)> = connection
            .exec(
                "select column_name, column_type, is_nullable, column_default, column_key \
                 from information_schema.columns where table_schema = ? and table_name = ? \
                 order by ordinal_position",
                (schema, table),
            )
            .await
            .map_err(|error| self.failed(&error))?;
        let index_rows: Vec<(String, i64, Option<String>)> = connection
            .exec(
                "select index_name, non_unique, column_name from information_schema.statistics \
                 where table_schema = ? and table_name = ? \
                 order by index_name = 'PRIMARY' desc, index_name, seq_in_index",
                (schema, table),
            )
            .await
            .map_err(|error| self.failed(&error))?;
        let key_rows: Vec<(String, String, Option<String>, String, String)> = connection
            .exec(
                "select constraint_name, column_name, referenced_table_schema, referenced_table_name, \
                        referenced_column_name \
                 from information_schema.key_column_usage \
                 where table_schema = ? and table_name = ? and referenced_table_name is not null \
                 order by constraint_name, ordinal_position",
                (schema, table),
            )
            .await
            .map_err(|error| self.failed(&error))?;
        let mut indexes: Vec<Index> = Vec::new();
        for (name, non_unique, column) in index_rows {
            let column = column.unwrap_or_else(|| "expression".into());
            match indexes.last_mut() {
                Some(index) if index.name == name => index.columns.push(column),
                _ => indexes.push(Index {
                    primary: name == "PRIMARY",
                    unique: non_unique == 0,
                    columns: vec![column],
                    name,
                }),
            }
        }
        let mut foreign_keys: Vec<ForeignKey> = Vec::new();
        for (name, column, references_schema, references_table, references_column) in key_rows {
            match foreign_keys.last_mut() {
                Some(key) if key.name.as_deref() == Some(name.as_str()) => {
                    key.columns.push(column);
                    key.references_columns.push(references_column);
                }
                _ => foreign_keys.push(ForeignKey {
                    name: Some(name),
                    columns: vec![column],
                    references_schema,
                    references_table,
                    references_columns: vec![references_column],
                }),
            }
        }
        Ok(TableInfo {
            schema: schema.to_string(),
            name: table.to_string(),
            kind: if kind.contains("VIEW") {
                TableKind::View
            } else {
                TableKind::Table
            },
            columns: columns
                .into_iter()
                .map(|(name, type_name, nullable, default, key)| ColumnInfo {
                    name,
                    type_name,
                    nullable: nullable == "YES",
                    default,
                    primary_key: key == "PRI",
                })
                .collect(),
            indexes,
            foreign_keys,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A server to test against, from `SIKEMUX_TEST_MYSQL` as `host:port/database/user/password`.
    /// Without it the tests that need a live server pass without running.
    pub fn server() -> Option<(Server, String)> {
        let given = std::env::var("SIKEMUX_TEST_MYSQL").ok()?;
        let mut parts = given.splitn(4, '/');
        let host_port = parts.next()?;
        let database = parts.next()?.to_string();
        let user = parts.next()?.to_string();
        let password = parts.next().unwrap_or_default().to_string();
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, "3306"));
        let server = Server {
            host: host.into(),
            port: Some(port.parse().ok()?),
            database,
            user,
            tls: Tls::Prefer,
        };
        Some((server, password))
    }

    /// A database of its own for one test, dropped when the test is done.
    pub struct Scratch {
        pub session: Session,
        pub schema: String,
    }

    impl Scratch {
        pub async fn new(name: &str, read_only: bool) -> Option<Self> {
            let (mut server, password) = server()?;
            let schema = format!("sikemux_{name}_{}", std::process::id());
            let setup = Session::open(&server, Some(&password), false)
                .await
                .unwrap();
            setup
                .query(
                    &format!(
                        "drop database if exists {schema};
                         create database {schema};
                         create table {schema}.customers (id int primary key auto_increment, name varchar(80) not null,
                             email varchar(120) unique);
                         create table {schema}.orders (id bigint primary key auto_increment,
                             customer_id int not null, total decimal(10,2) default 0, placed_at datetime null,
                             paid tinyint(1), note varbinary(16), big bigint unsigned,
                             index orders_by_customer (customer_id),
                             constraint orders_customer foreign key (customer_id) references {schema}.customers (id));
                         create view {schema}.big_orders as select * from {schema}.orders where total > 10;
                         insert into {schema}.customers (name, email) values ('Ada', 'ada@example.com'), ('Linus', null);
                         insert into {schema}.orders (customer_id, total, placed_at, paid, note, big)
                             values (1, 42.50, '2026-10-04 09:30:00', 1, x'cafe', 18446744073709551615),
                                    (1, 9.99, null, 0, null, 1);"
                    ),
                    1,
                )
                .await
                .unwrap();
            server.database = schema.clone();
            let session = Session::open(&server, Some(&password), read_only)
                .await
                .unwrap();
            Some(Self { session, schema })
        }

        pub async fn drop(self) {
            let _ = self
                .session
                .query(&format!("drop database if exists {}", self.schema), 1)
                .await;
        }
    }

    #[test]
    fn type_names_read_as_people_know_them() {
        assert_eq!(type_name(ColumnType::MYSQL_TYPE_LONGLONG), "longlong");
        assert_eq!(type_name(ColumnType::MYSQL_TYPE_NEWDECIMAL), "newdecimal");
        assert!(is_numeric(ColumnType::MYSQL_TYPE_NEWDECIMAL));
        assert!(!is_numeric(ColumnType::MYSQL_TYPE_VAR_STRING));
    }

    #[test]
    fn only_a_server_that_cannot_encrypt_is_reached_without_it() {
        assert!(lacks_tls(&mysql_async::Error::Driver(
            mysql_async::DriverError::NoClientSslFlagFromServer
        )));
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        assert!(!lacks_tls(&mysql_async::Error::Io(refused.into())));
    }

    #[tokio::test]
    async fn nobody_listening_is_a_connect_error() {
        let server = Server {
            host: "127.0.0.1".into(),
            port: Some(1),
            database: String::new(),
            user: "nobody".into(),
            tls: Tls::Disable,
        };
        assert!(matches!(
            Session::open(&server, None, false).await,
            Err(DatabaseError::Connect(_))
        ));
    }

    #[tokio::test]
    async fn a_live_server_names_itself_and_refuses_a_wrong_password() {
        let Some((server, password)) = server() else {
            return;
        };
        let session = Session::open(&server, Some(&password), false)
            .await
            .unwrap();
        assert!(session.version().await.unwrap().starts_with("MySQL 8"));
        let refused = Session::open(&server, Some("wrong"), false).await;
        let Err(DatabaseError::Connect(message)) = refused else {
            panic!("expected the sign-in to be refused")
        };
        assert!(message.contains("Access denied"), "{message}");
    }

    #[tokio::test]
    async fn a_live_query_returns_typed_cells_and_runs_every_statement() {
        let Some(scratch) = Scratch::new("query", false).await else {
            return;
        };
        let results = scratch
            .session
            .query(
                "select id, total, placed_at, paid, note, big from orders order by id; \
                 update orders set paid = 1; select count(*) as n from customers",
                10,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        let rows = &results[0].rows;
        assert_eq!(rows[0][0], serde_json::json!(1));
        assert_eq!(rows[0][1], serde_json::json!("42.50"));
        assert_eq!(rows[0][2], serde_json::json!("2026-10-04 09:30:00"));
        assert_eq!(rows[0][3], serde_json::json!(1));
        assert_eq!(rows[0][4], serde_json::json!("\\xcafe"));
        assert_eq!(rows[0][5], serde_json::json!("18446744073709551615"));
        assert_eq!(rows[1][2], serde_json::Value::Null);
        assert!(results[0].columns[1].numeric);
        assert_eq!(results[1].affected, Some(1));
        assert_eq!(results[2].rows, vec![vec![serde_json::json!(2)]]);
        let limited = scratch
            .session
            .query("select * from customers", 1)
            .await
            .unwrap();
        assert!(limited[0].truncated);
        assert_eq!(limited[0].rows.len(), 1);
        let setting: Option<u64> = scratch
            .session
            .connection
            .lock()
            .await
            .query_first("select @@session.sql_select_limit")
            .await
            .unwrap();
        assert_eq!(setting, Some(u64::MAX));
        let failed = scratch.session.query("select * from nowhere", 1).await;
        let Err(DatabaseError::Query(message)) = failed else {
            panic!("expected a query error")
        };
        assert!(message.contains("doesn't exist"), "{message}");
        assert!(scratch.session.is_alive());
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_live_server_lists_and_describes_tables() {
        let Some(scratch) = Scratch::new("describe", false).await else {
            return;
        };
        assert!(scratch
            .session
            .schemas()
            .await
            .unwrap()
            .contains(&scratch.schema));
        let tables = scratch.session.tables(&scratch.schema).await.unwrap();
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
        let orders = scratch
            .session
            .describe(&scratch.schema, "orders")
            .await
            .unwrap();
        assert_eq!(orders.columns[0].name, "id");
        assert!(orders.columns[0].primary_key);
        assert_eq!(orders.columns[2].type_name, "decimal(10,2)");
        assert_eq!(orders.columns[2].default.as_deref(), Some("0.00"));
        assert_eq!(orders.indexes[0].name, "PRIMARY");
        assert!(orders
            .indexes
            .iter()
            .any(|index| index.name == "orders_by_customer" && !index.unique));
        assert_eq!(
            orders.foreign_keys[0].name.as_deref(),
            Some("orders_customer")
        );
        assert_eq!(orders.foreign_keys[0].references_table, "customers");
        assert!(matches!(
            scratch.session.describe(&scratch.schema, "nope").await,
            Err(DatabaseError::NotFound(_))
        ));
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_guarded_query_cannot_switch_read_only_off() {
        let Some(scratch) = Scratch::new("guarded", true).await else {
            return;
        };
        let reader = &scratch.session;
        let escape = reader
            .query_guarded("set session transaction read write; delete from orders", 10)
            .await;
        let Err(DatabaseError::Query(message)) = escape else {
            panic!("expected the script to be refused")
        };
        assert!(message.contains("one statement at a time"), "{message}");
        reader
            .query_guarded("set session transaction read write", 10)
            .await
            .unwrap();
        for write in [
            "delete from orders",
            "create table sneaky (id int)",
            "drop table customers",
        ] {
            let refused = reader.query_guarded(write, 10).await;
            assert!(refused.is_err(), "{write} should be refused");
        }
        let read = reader
            .query_guarded("select count(*) from orders", 10)
            .await
            .unwrap();
        assert_eq!(read[0].rows, vec![vec![serde_json::json!(2)]]);
        let limited = reader
            .query_guarded("select * from customers", 1)
            .await
            .unwrap();
        assert!(limited[0].truncated);
        let names = reader.schemas().await.unwrap();
        assert!(names.len() > 1, "{names:?}");
        assert!(reader.is_alive());
        let (mut server, password) = server().unwrap();
        server.database = scratch.schema.clone();
        let cleanup = Scratch {
            session: Session::open(&server, Some(&password), false)
                .await
                .unwrap(),
            schema: scratch.schema,
        };
        let tables = cleanup.session.tables(&cleanup.schema).await.unwrap();
        assert_eq!(tables.len(), 3, "nothing was created or dropped");
        cleanup.drop().await;
    }

    #[tokio::test]
    async fn a_live_read_only_connection_refuses_changes_and_a_query_can_be_stopped() {
        let Some(scratch) = Scratch::new("readonly", true).await else {
            return;
        };
        let write = scratch.session.query("delete from orders", 1).await;
        let Err(DatabaseError::Query(message)) = write else {
            panic!("expected the change to be refused")
        };
        assert!(message.contains("READ ONLY"), "{message}");
        let running = scratch.session.clone();
        let sleeping = tokio::spawn(async move { running.query("select sleep(30)", 1).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        scratch.session.cancel().await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), sleeping)
            .await
            .unwrap()
            .unwrap();
        assert!(outcome.is_ok() || matches!(outcome, Err(DatabaseError::Query(_))));
        let (mut server, password) = server().unwrap();
        server.database = scratch.schema.clone();
        let cleanup = Scratch {
            session: Session::open(&server, Some(&password), false)
                .await
                .unwrap(),
            schema: scratch.schema,
        };
        cleanup.drop().await;
    }
}
