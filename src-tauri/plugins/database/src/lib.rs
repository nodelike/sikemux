// Databases: saved connections, their schemas, and SQL run against them.
//
//   agent       — the tools agents call: databases by name, tables, and SQL on a connection of their own
//   profiles    — the databases saved here, and their passwords in the Keychain
//   engines     — one open connection, whichever engine the database runs on
//   connections — trying a connection, and keeping one open per saved database
//   schema      — tables, columns, indexes and keys, the same for every engine
//   history     — the SQL run against each saved database, to find and run again
//   queries     — running SQL against a saved database, and stopping it
//   values      — query results, the same for every engine

mod agent;
mod connections;
mod engines;
mod error;
mod history;
mod profiles;
mod queries;
mod schema;
mod values;

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use sikemux_plugin_api::{
    params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
};

use crate::error::DatabaseResult;

pub fn plugin() -> Result<Arc<dyn Plugin>, PluginError> {
    Ok(Arc::new(Database {
        manifest: Manifest::from_json(include_str!("../manifest.json"))?,
        pool: connections::Pool::default(),
    }))
}

struct Database {
    manifest: Manifest,
    pool: connections::Pool,
}

async fn answer<T: Serialize>(
    result: impl std::future::Future<Output = DatabaseResult<T>>,
) -> Result<Value, PluginError> {
    reply(result.await?)
}

impl Plugin for Database {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir().to_path_buf();
            match method {
                "profiles" => reply(profiles::load(&data_dir)?.profiles),
                "save" => {
                    let request: profiles::SaveRequest = params(input)?;
                    let saved =
                        profiles::blocking(move || profiles::save(&data_dir, request)).await?;
                    self.pool.forget(&saved.id).await;
                    reply(saved)
                }
                "remove" => {
                    let profiles::IdRequest { id } = params(input)?;
                    self.pool.forget(&id).await;
                    answer(profiles::blocking(move || {
                        profiles::remove(&data_dir, &id)?;
                        history::clear(&data_dir, &id)
                    }))
                    .await
                }
                "test" => answer(connections::test(data_dir, params(input)?)).await,
                "connect" => {
                    let profiles::IdRequest { id } = params(input)?;
                    answer(self.pool.connect(&data_dir, &id)).await
                }
                "disconnect" => {
                    let profiles::IdRequest { id } = params(input)?;
                    self.pool.forget(&id).await;
                    reply(())
                }
                "connected" => reply(self.pool.connected().await),
                "schemas" => {
                    let profiles::IdRequest { id } = params(input)?;
                    answer(async {
                        self.pool
                            .session(&data_dir, &id, connections::Access::Browse)
                            .await?
                            .schemas()
                            .await
                    })
                    .await
                }
                "tables" => {
                    let schema::SchemaRequest { id, schema } = params(input)?;
                    answer(async {
                        self.pool
                            .session(&data_dir, &id, connections::Access::Browse)
                            .await?
                            .tables(schema)
                            .await
                    })
                    .await
                }
                "query" => {
                    let request = params(input)?;
                    answer(queries::run(
                        &self.pool,
                        &data_dir,
                        request,
                        history::Source::Person,
                    ))
                    .await
                }
                "agentDatabases" => reply(agent::databases(&data_dir)?),
                "agentTables" => answer(agent::tables(&self.pool, &data_dir, params(input)?)).await,
                "agentDescribe" => {
                    answer(agent::describe(&self.pool, &data_dir, params(input)?)).await
                }
                "agentQuery" => answer(agent::query(&self.pool, &data_dir, params(input)?)).await,
                "history" => {
                    let request: history::HistoryRequest = params(input)?;
                    answer(profiles::blocking(move || {
                        history::list(&data_dir, &request)
                    }))
                    .await
                }
                "clearHistory" => {
                    let profiles::IdRequest { id } = params(input)?;
                    answer(profiles::blocking(move || history::clear(&data_dir, &id))).await
                }
                "cancel" => answer(queries::cancel(&self.pool, params(input)?)).await,
                "describe" => {
                    let schema::TableRequest { id, schema, table } = params(input)?;
                    answer(async {
                        self.pool
                            .session(&data_dir, &id, connections::Access::Browse)
                            .await?
                            .describe(schema, table)
                            .await
                    })
                    .await
                }
                _ => Err(PluginError::unknown_method(method)),
            }
        })
    }

    fn offers_agent_tools<'a>(
        &'a self,
        ctx: &'a PluginContext,
        _remotes: &'a [String],
    ) -> PluginFuture<'a, bool> {
        Box::pin(async move { Ok(!profiles::load(ctx.data_dir())?.profiles.is_empty()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PluginContext {
        let dir = std::env::temp_dir().join(format!(
            "sikemux-database-plugin-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        PluginContext::new(dir)
    }

    #[tokio::test]
    async fn a_profile_is_saved_listed_and_removed_through_its_methods() {
        let ctx = scratch("methods");
        let database = plugin().unwrap();
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(false)
        );
        let saved = database
            .call(
                &ctx,
                "save",
                json!({ "profile": { "name": "Local", "engine": "sqlite", "path": "/tmp/x.db" } }),
            )
            .await
            .unwrap();
        let listed = database.call(&ctx, "profiles", Value::Null).await.unwrap();
        assert_eq!(listed, json!([saved.clone()]));
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(true)
        );
        database
            .call(&ctx, "remove", json!({ "id": saved["id"] }))
            .await
            .unwrap();
        let listed = database.call(&ctx, "profiles", Value::Null).await.unwrap();
        assert_eq!(listed, json!([]));
        assert!(database.call(&ctx, "nope", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn testing_a_connection_names_the_engine_and_its_version() {
        let ctx = scratch("test");
        let database = plugin().unwrap();
        let path = engines::sqlite::tests::fixture("plugin-test");
        let tested = database
            .call(
                &ctx,
                "test",
                json!({ "profile": { "name": "Local", "engine": "sqlite", "path": path } }),
            )
            .await
            .unwrap();
        assert!(tested["version"].as_str().unwrap().starts_with("SQLite"));
        let missing = database
            .call(
                &ctx,
                "test",
                json!({ "profile": { "name": "Gone", "engine": "sqlite", "path": "/nope/x.db" } }),
            )
            .await;
        assert_eq!(
            missing.err().map(|error| error.category),
            Some("connect".to_string())
        );
    }

    async fn saved_fixture(database: &Arc<dyn Plugin>, ctx: &PluginContext, name: &str) -> Value {
        let path = engines::sqlite::tests::fixture(name);
        let saved = database
            .call(
                ctx,
                "save",
                json!({ "profile": { "name": name, "engine": "sqlite", "path": path } }),
            )
            .await
            .unwrap();
        saved["id"].clone()
    }

    #[tokio::test]
    async fn the_schema_is_browsed_through_its_methods_connecting_on_first_use() {
        let ctx = scratch("browse");
        let database = plugin().unwrap();
        let id = saved_fixture(&database, &ctx, "plugin-browse").await;
        let schemas = database
            .call(&ctx, "schemas", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(schemas, json!(["main"]));
        let tables = database
            .call(&ctx, "tables", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(tables[1], json!({ "name": "customers", "kind": "table" }));
        let described = database
            .call(&ctx, "describe", json!({ "id": id, "table": "orders" }))
            .await
            .unwrap();
        assert_eq!(described["schema"], "main");
        assert_eq!(described["foreignKeys"][0]["referencesTable"], "customers");
        let connected = database.call(&ctx, "connected", Value::Null).await.unwrap();
        assert_eq!(connected[0]["id"], id);
        let missing = database
            .call(&ctx, "describe", json!({ "id": id, "table": "nope" }))
            .await;
        assert_eq!(
            missing.err().map(|error| error.category),
            Some("not-found".to_string())
        );
    }

    #[tokio::test]
    async fn sql_runs_through_its_method_and_empty_sql_is_refused() {
        let ctx = scratch("query");
        let database = plugin().unwrap();
        let id = saved_fixture(&database, &ctx, "plugin-query").await;
        let outcome = database
            .call(
                &ctx,
                "query",
                json!({ "id": id, "sql": "select name from customers order by id", "limit": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(outcome["results"][0]["rows"], json!([["Ada"]]));
        assert_eq!(outcome["results"][0]["truncated"], true);
        assert!(outcome["millis"].is_u64());
        let empty = database
            .call(&ctx, "query", json!({ "id": id, "sql": "  " }))
            .await;
        assert_eq!(
            empty.err().map(|error| error.category),
            Some("bad-params".to_string())
        );
        let cancelled = database
            .call(&ctx, "cancel", json!({ "id": "not-open", "run": "r1" }))
            .await;
        assert!(cancelled.is_ok());
    }

    #[tokio::test]
    async fn each_run_lands_in_the_history_with_its_outcome() {
        let ctx = scratch("history");
        let database = plugin().unwrap();
        let id = saved_fixture(&database, &ctx, "plugin-history").await;
        for sql in ["select * from customers", "select * from nowhere"] {
            let _ = database
                .call(&ctx, "query", json!({ "id": id, "sql": sql }))
                .await;
        }
        let history = database
            .call(&ctx, "history", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(history[0]["sql"], "select * from nowhere");
        assert_eq!(history[0]["ok"], false);
        assert!(history[0]["error"]
            .as_str()
            .unwrap()
            .contains("no such table"));
        assert_eq!(history[1]["rows"], 2);
        assert_eq!(history[1]["source"], "person");
        let found = database
            .call(&ctx, "history", json!({ "id": id, "search": "NOWHERE" }))
            .await
            .unwrap();
        assert_eq!(found.as_array().map(Vec::len), Some(1));
        database
            .call(&ctx, "remove", json!({ "id": id }))
            .await
            .unwrap();
        let history = database
            .call(&ctx, "history", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(history, json!([]));
    }

    #[tokio::test]
    async fn an_agent_finds_a_database_by_name_reads_it_and_cannot_change_it() {
        let ctx = scratch("agent");
        let database = plugin().unwrap();
        let id = saved_fixture(&database, &ctx, "Shop").await;
        let listed = database
            .call(&ctx, "agentDatabases", json!({}))
            .await
            .unwrap();
        assert_eq!(listed[0]["name"], "Shop");
        assert_eq!(listed[0]["engine"], "sqlite");
        assert_eq!(listed[0]["writable"], false);
        let tables = database
            .call(&ctx, "agentTables", json!({ "database": "shop" }))
            .await
            .unwrap();
        assert_eq!(tables["schema"], "main");
        assert_eq!(tables["tables"][2]["name"], "orders");
        let described = database
            .call(
                &ctx,
                "agentDescribe",
                json!({ "database": "Shop", "table": "customers" }),
            )
            .await
            .unwrap();
        assert_eq!(described["columns"][0]["name"], "id");
        let read = database
            .call(
                &ctx,
                "agentQuery",
                json!({ "database": "Shop", "sql": "select count(*) as n from orders" }),
            )
            .await
            .unwrap();
        assert_eq!(read["results"][0]["rows"], json!([[2]]));
        let write = database
            .call(
                &ctx,
                "agentQuery",
                json!({ "database": "Shop", "sql": "delete from orders" }),
            )
            .await;
        assert_eq!(
            write.err().map(|error| error.category),
            Some("query".to_string())
        );
        let history = database
            .call(&ctx, "history", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(history[0]["source"], "agent");
        assert_eq!(history[0]["ok"], false);
        let unknown = database
            .call(&ctx, "agentTables", json!({ "database": "Nope" }))
            .await;
        assert_eq!(
            unknown.err().map(|error| error.category),
            Some("not-found".to_string())
        );
    }

    #[tokio::test]
    async fn a_read_only_agent_on_postgres_runs_one_statement_at_a_time() {
        let Some((host, port, database_name, user)) = engines::postgres::tests::server() else {
            return;
        };
        let ctx = scratch("agent-guard");
        let database = plugin().unwrap();
        database
            .call(
                &ctx,
                "save",
                json!({ "profile": { "name": "Live", "engine": "postgres", "host": host, "port": port,
                                      "database": database_name, "user": user } }),
            )
            .await
            .unwrap();
        let escape = database
            .call(
                &ctx,
                "agentQuery",
                json!({ "database": "Live", "sql": "set default_transaction_read_only = off; create table sikemux_sneaky (id int)" }),
            )
            .await;
        let message = escape.err().map(|error| error.message).unwrap_or_default();
        assert!(message.contains("one statement at a time"), "{message}");
        let read = database
            .call(
                &ctx,
                "agentQuery",
                json!({ "database": "Live", "sql": "select 1 as one" }),
            )
            .await
            .unwrap();
        assert_eq!(read["results"][0]["rows"], json!([[1]]));
    }

    #[tokio::test]
    async fn editing_a_connected_profile_closes_its_connection() {
        let ctx = scratch("edit-closes");
        let database = plugin().unwrap();
        let path = engines::sqlite::tests::fixture("plugin-edit");
        let profile = json!({ "name": "Local", "engine": "sqlite", "path": path });
        let saved = database
            .call(&ctx, "save", json!({ "profile": profile }))
            .await
            .unwrap();
        let id = saved["id"].clone();
        database
            .call(&ctx, "connect", json!({ "id": id }))
            .await
            .unwrap();
        let connected = database.call(&ctx, "connected", Value::Null).await.unwrap();
        assert_eq!(connected[0]["id"], id);
        let mut edited = profile.clone();
        edited["id"] = id.clone();
        edited["name"] = json!("Renamed");
        database
            .call(&ctx, "save", json!({ "profile": edited }))
            .await
            .unwrap();
        let connected = database.call(&ctx, "connected", Value::Null).await.unwrap();
        assert_eq!(connected, json!([]));
    }
}
