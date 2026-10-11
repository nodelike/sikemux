import { resource } from "../../plugin-api/resources";
import { databaseApi, type Connected, type DatabaseProfile, type DatabaseTable, type HistoryEntry, type TableInfo } from "./api";

export const databaseProfilesR = resource({
    kind: "database.profiles",
    fetch: (): Promise<DatabaseProfile[]> => databaseApi.profiles(),
    staleAfterMs: 300_000,
});

export const databaseConnectedR = resource({
    kind: "database.connected",
    fetch: (): Promise<Connected[]> => databaseApi.connected(),
    staleAfterMs: 30_000,
});

export const databaseSchemasR = resource({
    kind: "database.schemas",
    fetch: (id: string): Promise<string[]> => databaseApi.schemas(id),
    staleAfterMs: 300_000,
});

/** An empty schema asks for the connection's default one. */
export const databaseTablesR = resource({
    kind: "database.tables",
    fetch: (id: string, schema: string): Promise<DatabaseTable[]> => databaseApi.tables(id, schema || undefined),
    staleAfterMs: 120_000,
});

export const databaseTableR = resource({
    kind: "database.table",
    fetch: (id: string, schema: string, table: string): Promise<TableInfo> => databaseApi.describe(id, table, schema || undefined),
    staleAfterMs: 120_000,
});

export const databaseHistoryR = resource({
    kind: "database.history",
    fetch: (id: string, search: string): Promise<HistoryEntry[]> => databaseApi.history(id, search || undefined),
    staleAfterMs: 10_000,
});
