// Databases: saved connections, their schemas, and SQL run against them.
//
//   agent       — the tools agents call: databases by name, tables, SQL on a connection of their own,
//                 and a database proposed for the person to approve
//   profiles    — the databases saved here, and their passwords in the Keychain
//   form        — a database's address read as a profile
//   engines     — one open connection, whichever engine the database runs on
//   connections — keeping one connection open per saved database
//   schema      — tables, columns, indexes and keys, the same for every engine
//   queries     — running SQL against a saved database
//   values      — query results, the same for every engine

mod agent;
mod connections;
mod engines;
mod error;
mod form;
mod profiles;
mod queries;
mod schema;
mod values;

use serde_json::Value;
use sikemux_plugin_api::{answer, params, reply, Plugin, PluginContext, PluginError, PluginFuture};

sikemux_plugin_api::plugin!(Database, pool: connections::Pool = connections::Pool::default());

impl Plugin for Database {
    fn call<'a>(
        &'a self,
        ctx: &'a PluginContext,
        method: &'a str,
        input: Value,
    ) -> PluginFuture<'a, Value> {
        Box::pin(async move {
            let data_dir = ctx.data_dir().to_path_buf();
            match method {
                "remove" => {
                    let agent::DatabaseRef { database } = params(input)?;
                    let id = profiles::load(&data_dir)?.find(&database)?.id.clone();
                    self.pool.forget(&id).await;
                    answer(profiles::blocking(move || profiles::remove(&data_dir, &id))).await
                }
                "setAgentWrites" => {
                    let agent::WritesRequest { database, on } = params(input)?;
                    let id = profiles::load(&data_dir)?.find(&database)?.id.clone();
                    self.pool.forget(&id).await;
                    answer(profiles::blocking(move || {
                        profiles::set_agent_writes(&data_dir, &id, on)
                    }))
                    .await
                }
                "setProduction" => {
                    let agent::WritesRequest { database, on } = params(input)?;
                    let id = profiles::load(&data_dir)?.find(&database)?.id.clone();
                    answer(profiles::blocking(move || {
                        profiles::set_production(&data_dir, &id, on)
                    }))
                    .await
                }
                "agentWritesProduction" => {
                    reply(agent::writes_production(&data_dir, params(input)?)?)
                }
                "agentDatabases" => reply(agent::databases(&data_dir)?),
                "agentTables" => answer(agent::tables(&self.pool, &data_dir, params(input)?)).await,
                "agentDescribe" => {
                    answer(agent::describe(&self.pool, &data_dir, params(input)?)).await
                }
                "agentQuery" => answer(agent::query(&self.pool, &data_dir, params(input)?)).await,
                "agentAddCheck" => answer(agent::add_check(&data_dir, params(input)?)).await,
                "agentAdd" => {
                    let request = params(input)?;
                    answer(profiles::blocking(move || agent::add(&data_dir, request))).await
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
    use std::sync::Arc;

    fn scratch(name: &str) -> PluginContext {
        let dir = std::env::temp_dir().join(format!(
            "sikemux-database-plugin-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        PluginContext::new(dir)
    }

    async fn saved_fixture(database: &Arc<dyn Plugin>, ctx: &PluginContext, name: &str) {
        let path = engines::sqlite::tests::fixture(name);
        let url = format!("sqlite://{}", path.display());
        database
            .call(ctx, "agentAdd", json!({ "url": url, "name": name }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_database_is_added_listed_and_removed_through_its_methods() {
        let ctx = scratch("methods");
        let database = plugin();
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(false)
        );
        saved_fixture(&database, &ctx, "Local").await;
        let listed = database
            .call(&ctx, "agentDatabases", json!({}))
            .await
            .unwrap();
        assert_eq!(listed[0]["name"], "Local");
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(true)
        );
        database
            .call(&ctx, "remove", json!({ "database": "Local" }))
            .await
            .unwrap();
        let listed = database
            .call(&ctx, "agentDatabases", json!({}))
            .await
            .unwrap();
        assert_eq!(listed, json!([]));
        assert!(database.call(&ctx, "nope", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn an_agent_finds_a_database_by_name_reads_it_and_cannot_change_it() {
        let ctx = scratch("agent");
        let database = plugin();
        saved_fixture(&database, &ctx, "Shop").await;
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
            Some(sikemux_plugin_api::Category::own("query"))
        );
        let unknown = database
            .call(&ctx, "agentTables", json!({ "database": "Nope" }))
            .await;
        assert_eq!(
            unknown.err().map(|error| error.category),
            Some(sikemux_plugin_api::Category::NotFound)
        );
    }

    #[tokio::test]
    async fn a_read_only_agent_on_postgres_runs_one_statement_at_a_time() {
        let Some((host, port, database_name, user)) = engines::postgres::tests::server() else {
            return;
        };
        let ctx = scratch("agent-guard");
        let database = plugin();
        let url = format!("postgres://{user}@{host}:{port}/{database_name}");
        database
            .call(&ctx, "agentAdd", json!({ "url": url, "name": "Live" }))
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
}
