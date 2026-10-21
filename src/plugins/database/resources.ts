import { pluginResources } from "../../plugin-api/service";
import { databaseApi } from "./api";
import { DATABASE_PLUGIN } from "./kinds";

export const database = pluginResources(DATABASE_PLUGIN);

export const databaseProfilesR = database.resource("profiles", () => databaseApi.profiles(), 300_000);

export const databaseConnectedR = database.resource("connected", () => databaseApi.connected(), 30_000);

export const databaseSchemasR = database.resource("schemas", (id: string) => databaseApi.schemas(id), 300_000);

/** An empty schema asks for the connection's default one. */
export const databaseTablesR = database.resource("tables", (id: string, schema: string) => databaseApi.tables(id, schema || undefined), 120_000);

export const databaseTableR = database.resource(
    "table",
    (id: string, schema: string, table: string) => databaseApi.describe(id, table, schema || undefined),
    120_000,
);

export const databaseHistoryR = database.resource("history", (id: string, search: string) => databaseApi.history(id, search || undefined), 10_000);
