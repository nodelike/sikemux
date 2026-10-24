// Databases: saved connections, their schemas, and SQL run against them.
//
//   agent       — the tools agents call: databases by name, tables, and SQL on a connection of their own
//   profiles    — the databases saved here, and their passwords in the Keychain
//   form        — the connection form, and a filled-in one read back as a profile
//   engines     — one open connection, whichever engine the database runs on
//   connections — trying a connection, and keeping one open per saved database
//   schema      — tables, columns, indexes and keys, the same for every engine
//   history     — the SQL run against each saved database, to find and run again
//   queries     — running SQL against a saved database, and stopping it
//   values      — query results, the same for every engine
//   results     — what the person's queries show, and their copies
//   views       — what the pane shows: the explorer, a table's structure and the history

mod agent;
mod connections;
mod engines;
mod error;
mod form;
mod history;
mod profiles;
mod queries;
mod results;
mod schema;
mod values;
mod views;

use std::sync::Arc;

use serde_json::Value;
use sikemux_plugin_api::{
    answer, params, reply, Manifest, Plugin, PluginContext, PluginError, PluginFuture,
};

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
                "form" => reply(form::form(&data_dir, params(input)?)?),
                "save" => {
                    let form::Filled { profile, password } = params(input)?;
                    let request = profiles::SaveRequest {
                        profile: form::profile(profile)?,
                        password,
                    };
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
                "test" => {
                    let form::Filled { profile, password } = params(input)?;
                    let request = connections::TestRequest {
                        profile: form::profile(profile)?,
                        password,
                    };
                    answer(connections::test(data_dir, request)).await
                }
                "connect" => {
                    let profiles::IdRequest { id } = params(input)?;
                    answer(self.pool.connect(&data_dir, &id)).await
                }
                "disconnect" => {
                    let profiles::IdRequest { id } = params(input)?;
                    self.pool.forget(&id).await;
                    reply(())
                }
                "query" => {
                    let request: queries::QueryRequest = params(input)?;
                    let (id, sql) = (request.id.clone(), request.sql.trim().to_owned());
                    let outcome =
                        queries::run(&self.pool, &data_dir, request, history::Source::Person)
                            .await?;
                    let saved = profiles::load(&data_dir)?.get(&id)?.clone();
                    let ran = results::Ran {
                        database: &saved.name,
                        engine: views::engine_name(&saved.target),
                        sql: &sql,
                    };
                    reply(results::shown(outcome, &ran))
                }
                "copyResult" => reply(results::copied(params(input)?)),
                "explorerView" => {
                    answer(views::explorer_view(&self.pool, data_dir, params(input)?)).await
                }
                "tableView" => {
                    answer(views::table_view(&self.pool, data_dir, params(input)?)).await
                }
                "historyView" => answer(views::history_view(data_dir, params(input)?)).await,
                "agentDatabases" => reply(agent::databases(&data_dir)?),
                "agentTables" => answer(agent::tables(&self.pool, &data_dir, params(input)?)).await,
                "agentDescribe" => {
                    answer(agent::describe(&self.pool, &data_dir, params(input)?)).await
                }
                "agentQuery" => answer(agent::query(&self.pool, &data_dir, params(input)?)).await,
                "clearHistory" => {
                    let profiles::IdRequest { id } = params(input)?;
                    answer(profiles::blocking(move || history::clear(&data_dir, &id))).await
                }
                "cancel" => answer(queries::cancel(&self.pool, params(input)?)).await,
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

    /// The form's draft for `id`, or its blank one, with `fields` typed in, as a pane sends it.
    async fn filled(
        database: &Arc<dyn Plugin>,
        ctx: &PluginContext,
        id: Option<&Value>,
        fields: Value,
    ) -> Value {
        let form = database
            .call(ctx, "form", json!({ "id": id }))
            .await
            .unwrap();
        let mut draft = form["draft"].clone();
        for (key, value) in fields.as_object().unwrap() {
            draft[key] = value.clone();
        }
        json!({ "profile": draft })
    }

    async fn explorer(database: &Arc<dyn Plugin>, ctx: &PluginContext, open: Value) -> Value {
        database
            .call(ctx, "explorerView", json!({ "open": open }))
            .await
            .unwrap()
    }

    async fn history(database: &Arc<dyn Plugin>, ctx: &PluginContext, ask: Value) -> Value {
        database.call(ctx, "historyView", ask).await.unwrap()["entries"].clone()
    }

    #[tokio::test]
    async fn a_profile_is_saved_listed_and_removed_through_its_methods() {
        let ctx = scratch("methods");
        let database = plugin().unwrap();
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(false)
        );
        let draft = filled(
            &database,
            &ctx,
            None,
            json!({ "name": "Local", "engine": "sqlite", "path": "/tmp/x.db" }),
        )
        .await;
        let saved = database.call(&ctx, "save", draft).await.unwrap();
        let listed = explorer(&database, &ctx, json!([])).await;
        assert_eq!(listed["connections"][0]["id"], saved["id"]);
        assert_eq!(listed["connections"][0]["name"], "Local");
        assert_eq!(
            database.offers_agent_tools(&ctx, &[]).await.ok(),
            Some(true)
        );
        database
            .call(&ctx, "remove", json!({ "id": saved["id"] }))
            .await
            .unwrap();
        let listed = explorer(&database, &ctx, json!([])).await;
        assert_eq!(listed["connections"], json!([]));
        assert!(database.call(&ctx, "nope", Value::Null).await.is_err());
    }

    #[tokio::test]
    async fn testing_a_connection_names_the_engine_and_its_version() {
        let ctx = scratch("test");
        let database = plugin().unwrap();
        let path = engines::sqlite::tests::fixture("plugin-test");
        let draft = filled(
            &database,
            &ctx,
            None,
            json!({ "name": "Local", "engine": "sqlite", "path": path }),
        )
        .await;
        let tested = database.call(&ctx, "test", draft).await.unwrap();
        assert!(tested["version"].as_str().unwrap().starts_with("SQLite"));
        let draft = filled(
            &database,
            &ctx,
            None,
            json!({ "name": "Gone", "engine": "sqlite", "path": "/nope/x.db" }),
        )
        .await;
        let missing = database.call(&ctx, "test", draft).await;
        assert_eq!(
            missing.err().map(|error| error.category),
            Some("connect".to_string())
        );
    }

    async fn saved_fixture(database: &Arc<dyn Plugin>, ctx: &PluginContext, name: &str) -> Value {
        let path = engines::sqlite::tests::fixture(name);
        let draft = filled(
            database,
            ctx,
            None,
            json!({ "name": name, "engine": "sqlite", "path": path }),
        )
        .await;
        let saved = database.call(ctx, "save", draft).await.unwrap();
        saved["id"].clone()
    }

    #[tokio::test]
    async fn the_schema_is_browsed_through_its_views_once_connected() {
        let ctx = scratch("browse");
        let database = plugin().unwrap();
        let id = saved_fixture(&database, &ctx, "plugin-browse").await;
        let open = json!([{ "id": id, "schemas": ["main"], "tables": [{ "schema": "main", "name": "orders" }] }]);
        let unconnected = explorer(&database, &ctx, open.clone()).await;
        assert_eq!(unconnected["connections"][0]["connected"], Value::Null);
        assert_eq!(unconnected["connections"][0]["schemas"], Value::Null);
        database
            .call(&ctx, "connect", json!({ "id": id }))
            .await
            .unwrap();
        let browsed = explorer(&database, &ctx, open).await;
        let connection = &browsed["connections"][0];
        assert!(connection["connected"].is_string());
        let schema = &connection["schemas"][0];
        assert_eq!(schema["name"], "main");
        assert_eq!(schema["groups"][0]["name"], "tables");
        assert_eq!(schema["groups"][0]["tables"][0]["name"], "customers");
        assert_eq!(schema["groups"][0]["tables"][0]["columns"], Value::Null);
        assert_eq!(
            schema["groups"][0]["tables"][1]["columns"][1]["name"],
            "customer_id"
        );
        assert_eq!(schema["groups"][1]["tables"][0]["name"], "big_orders");
        let described = database
            .call(&ctx, "tableView", json!({ "id": id, "table": "orders" }))
            .await
            .unwrap();
        assert_eq!(described["schema"], "main");
        assert_eq!(described["foreignKeys"][0]["table"], "customers");
        let missing = database
            .call(&ctx, "tableView", json!({ "id": id, "table": "nope" }))
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
        let entries = history(&database, &ctx, json!({ "id": id })).await;
        assert_eq!(entries[0]["sql"], "select * from nowhere");
        assert_eq!(entries[0]["ok"], false);
        assert!(entries[0]["error"]
            .as_str()
            .unwrap()
            .contains("no such table"));
        assert_eq!(entries[1]["rows"], 2);
        assert_eq!(entries[1]["byAgent"], false);
        let found = history(&database, &ctx, json!({ "id": id, "search": "NOWHERE" })).await;
        assert_eq!(found.as_array().map(Vec::len), Some(1));
        database
            .call(&ctx, "remove", json!({ "id": id }))
            .await
            .unwrap();
        let entries = history(&database, &ctx, json!({ "id": id })).await;
        assert_eq!(entries, json!([]));
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
        let entries = history(&database, &ctx, json!({ "id": id })).await;
        assert_eq!(entries[0]["byAgent"], true);
        assert_eq!(entries[0]["ok"], false);
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
        let draft = filled(
            &database,
            &ctx,
            None,
            json!({ "name": "Live", "engine": "postgres", "host": host, "port": port.to_string(),
                    "database": database_name, "user": user }),
        )
        .await;
        database.call(&ctx, "save", draft).await.unwrap();
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
        let id = saved_fixture(&database, &ctx, "plugin-edit").await;
        database
            .call(&ctx, "connect", json!({ "id": id }))
            .await
            .unwrap();
        let listed = explorer(&database, &ctx, json!([])).await;
        assert!(listed["connections"][0]["connected"].is_string());
        let edited = filled(&database, &ctx, Some(&id), json!({ "name": "Renamed" })).await;
        database.call(&ctx, "save", edited).await.unwrap();
        let listed = explorer(&database, &ctx, json!([])).await;
        assert_eq!(listed["connections"][0]["name"], "Renamed");
        assert_eq!(listed["connections"][0]["connected"], Value::Null);
    }
}
