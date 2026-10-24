import { pluginResources } from "../../plugin-api/service";
import { pluginView } from "../../plugin-api/views";
import { databaseApi } from "./api";
import { DATABASE_PLUGIN } from "./kinds";
import type { DatabaseExplorer, DatabaseHistory, DatabaseTableView } from "./types.generated";

export const database = pluginResources(DATABASE_PLUGIN);

/** The connection form for the saved connection with this id, or for a new one when it is empty. */
export const databaseFormR = database.resource("form", (id: string) => databaseApi.form(id || undefined), 0);

/** What the explorer has open in one connection: its schemas, and the tables whose columns show. */
export interface OpenNodes {
    id: string;
    schemas: string[];
    tables: { schema: string; name: string }[];
}

/** Every saved database, with what the explorer has open in each, kept by the core. */
export const databaseExplorerV = pluginView<[open: OpenNodes[]], DatabaseExplorer>(DATABASE_PLUGIN, "explorerView", (open) => ({ open }));

/** One table's structure; an empty schema asks for the connection's default one. */
export const databaseTableV = pluginView<[id: string, schema: string, table: string], DatabaseTableView>(
    DATABASE_PLUGIN,
    "tableView",
    (id, schema, table) => ({ id, schema: schema || undefined, table }),
);

/** The queries run on a database, newest first, those matching `search` when there is one. */
export const databaseHistoryV = pluginView<[id: string, search: string], DatabaseHistory>(DATABASE_PLUGIN, "historyView", (id, search) => ({
    id,
    search: search || undefined,
}));
