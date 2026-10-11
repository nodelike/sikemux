// PostgreSQL over the network, with the client kept open between calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres::types::Type;
use tokio_postgres::{Client, Config, SimpleQueryMessage};

use super::{tls, Unfinished};
use crate::error::{DatabaseError, DatabaseResult};
use crate::profiles::{Server, Tls, POSTGRES_PORT};
use crate::schema::{ColumnInfo, ForeignKey, Index, Table, TableInfo, TableKind};
use crate::values::{self, Column, ResultSet};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Session {
    client: Arc<Client>,
    /// Keeps one guarded query's transaction from interleaving with another's on the same connection.
    guard: Arc<tokio::sync::Mutex<()>>,
    /// Asking the server to stop a query goes over a new connection, encrypted the same way.
    tls: Tls,
    alive: Arc<AtomicBool>,
}

/// PostgreSQL's own words for a failure, with its detail and hint, rather than the bare "db error".
pub fn describe(error: &tokio_postgres::Error) -> String {
    if let Some(db) = error.as_db_error() {
        let mut text = db.message().to_string();
        if let Some(detail) = db.detail() {
            text.push_str(&format!("\n{detail}"));
        }
        if let Some(hint) = db.hint() {
            text.push_str(&format!("\nHint: {hint}"));
        }
        return text;
    }
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

fn query_error(error: tokio_postgres::Error) -> DatabaseError {
    DatabaseError::Query(describe(&error))
}

const SCHEMAS: &str = "select nspname::text from pg_namespace \
     where nspname not in ('pg_catalog', 'information_schema', 'pg_toast') \
       and nspname not like 'pg_temp_%' and nspname not like 'pg_toast_temp_%' \
       and has_schema_privilege(oid, 'USAGE') \
     order by nspname = 'public' desc, nspname";

const TABLES: &str = "select c.relname::text, c.relkind::text from pg_class c \
     join pg_namespace n on n.oid = c.relnamespace \
     where n.nspname = $1 and c.relkind in ('r', 'p', 'v', 'm', 'f') \
     order by c.relname";

const KIND: &str = "select c.relkind::text from pg_class c \
     join pg_namespace n on n.oid = c.relnamespace \
     where n.nspname = $1 and c.relname = $2 and c.relkind in ('r', 'p', 'v', 'm', 'f')";

const COLUMNS: &str =
    "select a.attname::text, format_type(a.atttypid, a.atttypmod), not a.attnotnull, \
            pg_get_expr(d.adbin, d.adrelid), coalesce(a.attnum = any(i.indkey), false) \
     from pg_attribute a \
     join pg_class c on c.oid = a.attrelid \
     join pg_namespace n on n.oid = c.relnamespace \
     left join pg_attrdef d on d.adrelid = a.attrelid and d.adnum = a.attnum \
     left join pg_index i on i.indrelid = c.oid and i.indisprimary \
     where n.nspname = $1 and c.relname = $2 and a.attnum > 0 and not a.attisdropped \
     order by a.attnum";

const INDEXES: &str = "select ic.relname::text, i.indisunique, i.indisprimary, \
            array(select coalesce(a.attname::text, pg_get_indexdef(i.indexrelid, k.ord::int, true)) \
                  from unnest(i.indkey) with ordinality k(attnum, ord) \
                  left join pg_attribute a on a.attrelid = i.indrelid and a.attnum = k.attnum \
                  order by k.ord) \
     from pg_index i \
     join pg_class ic on ic.oid = i.indexrelid \
     join pg_class c on c.oid = i.indrelid \
     join pg_namespace n on n.oid = c.relnamespace \
     where n.nspname = $1 and c.relname = $2 \
     order by i.indisprimary desc, ic.relname";

const FOREIGN_KEYS: &str = "select con.conname::text, \
            array(select a.attname::text from unnest(con.conkey) with ordinality k(num, ord) \
                  join pg_attribute a on a.attrelid = con.conrelid and a.attnum = k.num order by k.ord), \
            fn.nspname::text, fc.relname::text, \
            array(select a.attname::text from unnest(con.confkey) with ordinality k(num, ord) \
                  join pg_attribute a on a.attrelid = con.confrelid and a.attnum = k.num order by k.ord) \
     from pg_constraint con \
     join pg_class c on c.oid = con.conrelid \
     join pg_namespace n on n.oid = c.relnamespace \
     join pg_class fc on fc.oid = con.confrelid \
     join pg_namespace fn on fn.oid = fc.relnamespace \
     where con.contype = 'f' and n.nspname = $1 and c.relname = $2 \
     order by con.conname";

fn is_numeric(ty: &Type) -> bool {
    matches!(
        *ty,
        Type::INT2
            | Type::INT4
            | Type::INT8
            | Type::OID
            | Type::FLOAT4
            | Type::FLOAT8
            | Type::NUMERIC
            | Type::MONEY
    )
}

/// A value as PostgreSQL writes it in text, turned back into what its type means.
/// `numeric` stays text so no digit is lost; a type that is not known stays text too.
fn cell(text: Option<&str>, ty: Option<&Type>) -> serde_json::Value {
    let Some(text) = text else {
        return serde_json::Value::Null;
    };
    match ty {
        Some(&Type::BOOL) => serde_json::Value::Bool(text == "t"),
        Some(&Type::INT2 | &Type::INT4 | &Type::INT8 | &Type::OID) => text
            .parse()
            .map_or_else(|_| values::text(text), values::integer),
        Some(&Type::FLOAT4 | &Type::FLOAT8) => text
            .parse()
            .map_or_else(|_| values::text(text), values::real),
        _ => values::text(text),
    }
}

/// Builds results from the stream of messages a simple query sends: a row description opens a result,
/// rows fill it, and each statement's completion closes it with the count it reports.
#[derive(Default)]
struct Collector {
    results: Vec<ResultSet>,
    open: Option<ResultSet>,
    types: Option<Vec<Type>>,
}

impl Collector {
    /// Takes the next message, and says whether it was a row past the limit.
    fn take(&mut self, message: SimpleQueryMessage, limit: usize) -> bool {
        match message {
            SimpleQueryMessage::RowDescription(described) => {
                let types = self.types.take();
                let columns = described
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let ty = types.as_ref().and_then(|types| types.get(index));
                        Column {
                            name: column.name().to_string(),
                            type_name: ty.map(|ty| ty.name().to_string()).unwrap_or_default(),
                            numeric: ty.is_some_and(is_numeric),
                        }
                    })
                    .collect();
                self.open = Some(ResultSet {
                    columns,
                    ..ResultSet::default()
                });
                self.types = types;
            }
            SimpleQueryMessage::Row(row) => {
                let Some(open) = self.open.as_mut() else {
                    return false;
                };
                if open.rows.len() == limit {
                    open.truncated = true;
                    return true;
                }
                let types = self.types.as_ref();
                open.rows.push(
                    (0..row.len())
                        .map(|index| cell(row.get(index), types.and_then(|types| types.get(index))))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(_) if self.open.is_some() => self.close(),
            SimpleQueryMessage::CommandComplete(count) => self.results.push(ResultSet {
                affected: Some(count),
                ..ResultSet::default()
            }),
            _ => {}
        }
        false
    }

    /// Ends the result being filled, if there is one.
    fn close(&mut self) {
        let Some(mut finished) = self.open.take() else {
            return;
        };
        if self.types.is_none() {
            mark_numbers_by_text(&mut finished);
        }
        self.types = None;
        self.results.push(finished);
    }

    fn finish(mut self) -> Vec<ResultSet> {
        self.close();
        self.results
    }
}

/// Without the column types, a column whose every value reads as a number is treated as numbers.
fn mark_numbers_by_text(result: &mut ResultSet) {
    for (index, column) in result.columns.iter_mut().enumerate() {
        let mut texts = result
            .rows
            .iter()
            .filter_map(|row| row.get(index).and_then(serde_json::Value::as_str))
            .peekable();
        column.numeric = texts.peek().is_some() && texts.all(|text| text.parse::<f64>().is_ok());
    }
}

fn column_types(statement: &tokio_postgres::Statement) -> Vec<Type> {
    statement
        .columns()
        .iter()
        .map(|column| column.type_().clone())
        .collect()
}

fn ssl_mode(tls: Tls) -> SslMode {
    match tls {
        Tls::Disable => SslMode::Disable,
        Tls::Prefer => SslMode::Prefer,
        Tls::Require | Tls::VerifyFull => SslMode::Require,
    }
}

impl Session {
    pub async fn open(
        address: &Server,
        password: Option<&str>,
        read_only: bool,
    ) -> DatabaseResult<Self> {
        let mut config = Config::new();
        config
            .host(&address.host)
            .port(address.port.unwrap_or(POSTGRES_PORT))
            .user(&address.user)
            .application_name("Sikemux")
            .connect_timeout(CONNECT_TIMEOUT)
            .ssl_mode(ssl_mode(address.tls));
        if !address.database.is_empty() {
            config.dbname(&address.database);
        }
        if let Some(password) = password.filter(|password| !password.is_empty()) {
            config.password(password);
        }
        let connector = tls::connector(address.tls == Tls::VerifyFull)?;
        let connecting = tokio::time::timeout(CONNECT_TIMEOUT, config.connect(connector));
        let (client, connection) = connecting
            .await
            .map_err(|_| {
                DatabaseError::Connect(format!(
                    "{}:{} did not answer within {}s",
                    address.host,
                    address.port.unwrap_or(POSTGRES_PORT),
                    CONNECT_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|error| DatabaseError::Connect(describe(&error)))?;
        tokio::spawn(connection);
        if read_only {
            client
                .batch_execute("set default_transaction_read_only = on")
                .await
                .map_err(|error| DatabaseError::Connect(describe(&error)))?;
        }
        Ok(Self {
            client: Arc::new(client),
            guard: Arc::new(tokio::sync::Mutex::new(())),
            tls: address.tls,
            alive: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn is_alive(&self) -> bool {
        !self.client.is_closed() && self.alive.load(Ordering::Relaxed)
    }

    pub fn retire(&self) {
        self.alive.store(false, Ordering::Relaxed);
    }

    /// Runs every statement in the text, keeping at most `limit` rows from each. A single statement is
    /// prepared first so its columns come back with their types; a script of several comes back as text.
    pub async fn query(&self, sql: &str, limit: usize) -> DatabaseResult<Vec<ResultSet>> {
        let types = match self.client.prepare(sql).await {
            Ok(statement) => Some(column_types(&statement)),
            Err(_) => None,
        };
        self.collect(sql, types, limit, false).await
    }

    /// For an agent on a read-only connection. A session setting could otherwise switch read-only off for the
    /// statements after it, so the SQL must be one statement, and it runs in a read-only transaction that is
    /// always rolled back, taking anything it set with it. Since nothing it does is kept, it is stopped as soon
    /// as it has given `limit` rows. A connection the rollback did not reach is not used again.
    pub async fn query_guarded(&self, sql: &str, limit: usize) -> DatabaseResult<Vec<ResultSet>> {
        let _one_at_a_time = self.guard.lock().await;
        let statement = self.client.prepare(sql).await.map_err(|error| {
            let message = describe(&error);
            if message.contains("multiple commands") {
                DatabaseError::Query(
                    "on a read-only connection an agent runs one statement at a time".into(),
                )
            } else {
                DatabaseError::Query(message)
            }
        })?;
        let types = column_types(&statement);
        let unfinished = Unfinished::new(&self.alive);
        self.client
            .batch_execute("begin read only")
            .await
            .map_err(query_error)?;
        let outcome = self.collect(sql, Some(types), limit, true).await;
        let rolled_back = self
            .client
            .batch_execute("rollback")
            .await
            .map_err(query_error);
        if rolled_back.is_ok() {
            unfinished.finish();
        }
        let results = outcome?;
        rolled_back?;
        Ok(results)
    }

    /// With `stop_at_limit`, a statement that gives more than `limit` rows is stopped on the server rather
    /// than read to its end.
    async fn collect(
        &self,
        sql: &str,
        types: Option<Vec<Type>>,
        limit: usize,
        stop_at_limit: bool,
    ) -> DatabaseResult<Vec<ResultSet>> {
        let mut collector = Collector {
            types,
            ..Collector::default()
        };
        let stream = self
            .client
            .simple_query_raw(sql)
            .await
            .map_err(query_error)?;
        let mut stream = std::pin::pin!(stream);
        let mut stopped = false;
        while let Some(message) = stream.next().await {
            match message {
                Ok(message) => {
                    if collector.take(message, limit) && stop_at_limit && !stopped {
                        stopped = true;
                        self.cancel().await?;
                    }
                }
                Err(error) if stopped && error.code() == Some(&SqlState::QUERY_CANCELED) => break,
                Err(error) => return Err(query_error(error)),
            }
        }
        Ok(collector.finish())
    }

    /// Asks the server to stop whatever this connection is running; the query ends with an error saying so.
    pub async fn cancel(&self) -> DatabaseResult<()> {
        let connector = tls::connector(self.tls == Tls::VerifyFull)?;
        self.client
            .cancel_token()
            .cancel_query(connector)
            .await
            .map_err(|error| DatabaseError::Connect(describe(&error)))
    }

    /// The schemas this user may look into, `public` first.
    pub async fn schemas(&self) -> DatabaseResult<Vec<String>> {
        let rows = self.client.query(SCHEMAS, &[]).await.map_err(query_error)?;
        Ok(rows.iter().map(|row| row.get(0)).collect())
    }

    pub async fn tables(&self, schema: &str) -> DatabaseResult<Vec<Table>> {
        let rows = self
            .client
            .query(TABLES, &[&schema])
            .await
            .map_err(query_error)?;
        Ok(rows
            .iter()
            .map(|row| Table {
                name: row.get(0),
                kind: TableKind::from_postgres(row.get(1)),
            })
            .collect())
    }

    pub async fn describe(&self, schema: &str, table: &str) -> DatabaseResult<TableInfo> {
        let params: [&(dyn tokio_postgres::types::ToSql + Sync); 2] = [&schema, &table];
        let kind = self
            .client
            .query_opt(KIND, &params)
            .await
            .map_err(query_error)?
            .ok_or_else(|| {
                DatabaseError::NotFound(format!("there is no table or view named {schema}.{table}"))
            })?;
        let columns = self
            .client
            .query(COLUMNS, &params)
            .await
            .map_err(query_error)?;
        let indexes = self
            .client
            .query(INDEXES, &params)
            .await
            .map_err(query_error)?;
        let foreign_keys = self
            .client
            .query(FOREIGN_KEYS, &params)
            .await
            .map_err(query_error)?;
        Ok(TableInfo {
            schema: schema.to_string(),
            name: table.to_string(),
            kind: TableKind::from_postgres(kind.get(0)),
            columns: columns
                .iter()
                .map(|row| ColumnInfo {
                    name: row.get(0),
                    type_name: row.get(1),
                    nullable: row.get(2),
                    default: row.get(3),
                    primary_key: row.get(4),
                })
                .collect(),
            indexes: indexes
                .iter()
                .map(|row| Index {
                    name: row.get(0),
                    unique: row.get(1),
                    primary: row.get(2),
                    columns: row.get(3),
                })
                .collect(),
            foreign_keys: foreign_keys
                .iter()
                .map(|row| ForeignKey {
                    name: row.get(0),
                    columns: row.get(1),
                    references_schema: row.get(2),
                    references_table: row.get(3),
                    references_columns: row.get(4),
                })
                .collect(),
        })
    }

    pub async fn version(&self) -> DatabaseResult<String> {
        let row = self
            .client
            .query_one("show server_version", &[])
            .await
            .map_err(|error| DatabaseError::Query(describe(&error)))?;
        let version: String = row.get(0);
        Ok(format!("PostgreSQL {version}"))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A server to test against, from `SIKEMUX_TEST_POSTGRES` as `host:port/database/user`.
    /// Without it the tests that need a live server pass without running.
    pub fn server() -> Option<(String, u16, String, String)> {
        let given = std::env::var("SIKEMUX_TEST_POSTGRES").ok()?;
        let (host_port, rest) = given.split_once('/')?;
        let (database, user) = rest.split_once('/')?;
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, "5432"));
        Some((
            host.into(),
            port.parse().ok()?,
            database.into(),
            user.into(),
        ))
    }

    pub async fn open_test_server(read_only: bool) -> Option<Session> {
        let (host, port, database, user) = server()?;
        let address = Server {
            host,
            port: Some(port),
            database,
            user,
            tls: Tls::Prefer,
        };
        Some(Session::open(&address, None, read_only).await.unwrap())
    }

    /// A schema of its own for one test, with two related tables and a view, dropped when the test is done.
    pub struct Scratch {
        pub session: Session,
        pub schema: String,
    }

    impl Scratch {
        pub async fn new(name: &str) -> Option<Self> {
            let session = open_test_server(false).await?;
            let schema = format!("sikemux_{name}_{}", std::process::id());
            session
                .client
                .batch_execute(&format!(
                    "drop schema if exists {schema} cascade;
                     create schema {schema};
                     create table {schema}.customers (id serial primary key, name text not null, email text unique);
                     create table {schema}.orders (
                         id bigserial primary key,
                         customer_id int not null references {schema}.customers(id),
                         total numeric(10, 2) default 0,
                         placed_at timestamptz,
                         paid boolean,
                         note bytea,
                         tags text[]
                     );
                     create index orders_by_customer on {schema}.orders (customer_id, lower(coalesce(tags[1], '')));
                     create view {schema}.big_orders as select * from {schema}.orders where total > 10;
                     insert into {schema}.customers (name, email) values ('Ada', 'ada@example.com'), ('Linus', null);
                     insert into {schema}.orders (customer_id, total, placed_at, paid, note, tags)
                         values (1, 42.50, '2026-10-04 09:30:00+00', true, '\\xcafe', array['rush']),
                                (1, 9.99, null, false, null, null);"
                ))
                .await
                .unwrap();
            Some(Self { session, schema })
        }

        pub async fn drop(self) {
            let _ = self
                .session
                .client
                .batch_execute(&format!("drop schema if exists {} cascade", self.schema))
                .await;
        }
    }

    #[tokio::test]
    async fn a_live_server_lists_schemas_and_tables() {
        let Some(scratch) = Scratch::new("tables").await else {
            return;
        };
        let schemas = scratch.session.schemas().await.unwrap();
        assert_eq!(schemas.first().map(String::as_str), Some("public"));
        assert!(schemas.contains(&scratch.schema));
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
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_live_server_describes_a_table_with_its_keys_and_indexes() {
        let Some(scratch) = Scratch::new("describe").await else {
            return;
        };
        let orders = scratch
            .session
            .describe(&scratch.schema, "orders")
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
        assert_eq!(columns[0], ("id", "bigint", false, true));
        assert_eq!(columns[1], ("customer_id", "integer", false, false));
        assert_eq!(columns[2], ("total", "numeric(10,2)", true, false));
        assert_eq!(columns[6], ("tags", "text[]", true, false));
        assert!(orders.columns[0]
            .default
            .as_deref()
            .unwrap_or_default()
            .starts_with("nextval("));
        assert_eq!(orders.indexes[0].name, "orders_pkey");
        assert!(orders.indexes[0].primary);
        let by_customer = orders
            .indexes
            .iter()
            .find(|index| index.name == "orders_by_customer")
            .unwrap();
        assert_eq!(by_customer.columns[0], "customer_id");
        assert!(
            by_customer.columns[1].contains("lower"),
            "{:?}",
            by_customer.columns
        );
        assert_eq!(orders.foreign_keys.len(), 1);
        assert_eq!(
            orders.foreign_keys[0].columns,
            vec!["customer_id".to_string()]
        );
        assert_eq!(orders.foreign_keys[0].references_table, "customers");
        assert_eq!(
            orders.foreign_keys[0].references_schema.as_deref(),
            Some(scratch.schema.as_str())
        );
        let view = scratch
            .session
            .describe(&scratch.schema, "big_orders")
            .await
            .unwrap();
        assert_eq!(view.kind, TableKind::View);
        assert!(matches!(
            scratch.session.describe(&scratch.schema, "nope").await,
            Err(DatabaseError::NotFound(_))
        ));
        scratch.drop().await;
    }

    #[test]
    fn text_values_become_what_their_types_mean() {
        assert_eq!(cell(None, Some(&Type::INT4)), serde_json::Value::Null);
        assert_eq!(cell(Some("t"), Some(&Type::BOOL)), serde_json::json!(true));
        assert_eq!(cell(Some("42"), Some(&Type::INT8)), serde_json::json!(42));
        assert_eq!(
            cell(Some("9223372036854775807"), Some(&Type::INT8)),
            serde_json::json!("9223372036854775807")
        );
        assert_eq!(
            cell(Some("1.5"), Some(&Type::FLOAT8)),
            serde_json::json!(1.5)
        );
        assert_eq!(
            cell(Some("NaN"), Some(&Type::FLOAT8)),
            serde_json::json!("NaN")
        );
        assert_eq!(
            cell(Some("12345678901234567890.12"), Some(&Type::NUMERIC)),
            serde_json::json!("12345678901234567890.12")
        );
        assert_eq!(cell(Some("{a,b}"), None), serde_json::json!("{a,b}"));
    }

    #[tokio::test]
    async fn a_live_query_returns_typed_cells() {
        let Some(scratch) = Scratch::new("query").await else {
            return;
        };
        let sql = format!(
            "select id, customer_id, total, placed_at, paid, note, tags from {}.orders order by id",
            scratch.schema
        );
        let results = scratch.session.query(&sql, 100).await.unwrap();
        assert_eq!(results.len(), 1);
        let result = &results[0];
        let types: Vec<(&str, bool)> = result
            .columns
            .iter()
            .map(|column| (column.type_name.as_str(), column.numeric))
            .collect();
        assert_eq!(
            types,
            vec![
                ("int8", true),
                ("int4", true),
                ("numeric", true),
                ("timestamptz", false),
                ("bool", false),
                ("bytea", false),
                ("_text", false)
            ]
        );
        assert_eq!(result.rows[0][0], serde_json::json!(1));
        assert_eq!(result.rows[0][2], serde_json::json!("42.50"));
        assert_eq!(result.rows[0][4], serde_json::json!(true));
        assert_eq!(result.rows[0][5], serde_json::json!("\\xcafe"));
        assert_eq!(result.rows[0][6], serde_json::json!("{rush}"));
        assert_eq!(result.rows[1][3], serde_json::Value::Null);
        assert_eq!(result.affected, None);
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_live_script_reports_each_statement_and_keeps_to_the_limit() {
        let Some(scratch) = Scratch::new("script").await else {
            return;
        };
        let schema = &scratch.schema;
        let sql = format!(
            "update {schema}.orders set paid = true; select name from {schema}.customers order by id; select count(*) from {schema}.orders"
        );
        let results = scratch.session.query(&sql, 1).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].affected, Some(2));
        assert_eq!(results[1].rows, vec![vec![serde_json::json!("Ada")]]);
        assert!(results[1].truncated);
        assert!(results[2].columns[0].numeric);
        let failed = scratch.session.query("select * from nowhere", 1).await;
        let Err(DatabaseError::Query(message)) = failed else {
            panic!("expected a query error")
        };
        assert!(message.contains("does not exist"), "{message}");
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_guarded_query_cannot_switch_read_only_off() {
        let Some(scratch) = Scratch::new("guarded").await else {
            return;
        };
        let reader = open_test_server(true).await.unwrap();
        let schema = &scratch.schema;
        let escape =
            format!("set default_transaction_read_only = off; delete from {schema}.orders");
        let refused = reader.query_guarded(&escape, 10).await;
        let Err(DatabaseError::Query(message)) = refused else {
            panic!("expected the script to be refused")
        };
        assert!(message.contains("one statement at a time"), "{message}");
        let committed = reader
            .query_guarded(&format!("commit; delete from {schema}.orders"), 10)
            .await;
        assert!(committed.is_err());
        let setting = reader
            .query_guarded("set session characteristics as transaction read write", 10)
            .await;
        assert!(setting.is_ok(), "{:?}", setting.err());
        let write = reader
            .query_guarded(&format!("delete from {schema}.orders"), 10)
            .await;
        let Err(DatabaseError::Query(message)) = write else {
            panic!("expected the delete to be refused")
        };
        assert!(message.contains("read-only"), "{message}");
        let read = reader
            .query_guarded(&format!("select count(*) from {schema}.orders"), 10)
            .await
            .unwrap();
        assert_eq!(read[0].rows, vec![vec![serde_json::json!(2)]]);
        assert!(reader
            .query_guarded("select * from nowhere", 1)
            .await
            .is_err());
        assert!(reader.query_guarded("select 1", 1).await.is_ok());
        scratch.drop().await;
    }

    #[tokio::test]
    async fn a_guarded_query_stops_on_the_server_at_its_limit() {
        let Some(reader) = open_test_server(true).await else {
            return;
        };
        let started = std::time::Instant::now();
        let results = reader
            .query_guarded("select n from generate_series(1, 100000000) n", 3)
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].rows.len(), 3);
        assert!(results[0].truncated);
        assert!(reader.is_alive());
        let next = reader.query_guarded("select 1", 1).await.unwrap();
        assert_eq!(next[0].rows, vec![vec![serde_json::json!(1)]]);
    }

    #[tokio::test]
    async fn a_guarded_query_dropped_halfway_retires_its_connection() {
        let Some(reader) = open_test_server(true).await else {
            return;
        };
        let dropped = tokio::time::timeout(
            Duration::from_millis(300),
            reader.query_guarded("select pg_sleep(30)", 1),
        )
        .await;
        assert!(dropped.is_err());
        assert!(!reader.is_alive());
    }

    #[tokio::test]
    async fn a_live_query_can_be_cancelled() {
        let Some(scratch) = Scratch::new("cancel").await else {
            return;
        };
        let running = scratch.session.clone();
        let sleeping = tokio::spawn(async move { running.query("select pg_sleep(30)", 1).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        scratch.session.cancel().await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), sleeping)
            .await
            .unwrap()
            .unwrap();
        let Err(DatabaseError::Query(message)) = outcome else {
            panic!("expected the query to be cancelled")
        };
        assert!(message.contains("cancel"), "{message}");
        scratch.drop().await;
    }

    #[test]
    fn modes_map_to_postgres_ssl_modes() {
        assert!(matches!(ssl_mode(Tls::Disable), SslMode::Disable));
        assert!(matches!(ssl_mode(Tls::Prefer), SslMode::Prefer));
        assert!(matches!(ssl_mode(Tls::Require), SslMode::Require));
        assert!(matches!(ssl_mode(Tls::VerifyFull), SslMode::Require));
    }

    #[tokio::test]
    async fn nobody_listening_is_a_connect_error_naming_the_cause() {
        let address = Server {
            host: "127.0.0.1".into(),
            port: Some(1),
            database: String::new(),
            user: "nobody".into(),
            tls: Tls::Disable,
        };
        let failed = Session::open(&address, None, false).await;
        let Err(DatabaseError::Connect(message)) = failed else {
            panic!("expected a connect error");
        };
        assert!(message.to_lowercase().contains("refused"), "{message}");
    }

    #[tokio::test]
    async fn a_live_server_reports_its_version_and_keeps_read_only() {
        let Some(session) = open_test_server(true).await else {
            return;
        };
        assert!(session.version().await.unwrap().starts_with("PostgreSQL "));
        let write = session
            .client
            .batch_execute("create temporary table sikemux_probe (id int)")
            .await;
        let error = write
            .err()
            .map(|error| describe(&error))
            .unwrap_or_default();
        assert!(error.contains("read-only"), "{error}");
    }
}
