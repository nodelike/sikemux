import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { DATABASE_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(DATABASE_PLUGIN_ID);

/** PostgreSQL's `sslmode` names: whether to encrypt, and whether to check the server's certificate. */
export type TlsMode = "disable" | "prefer" | "require" | "verify-full";

/** A database server reached over the network; an empty port means the engine's usual one. */
export interface ServerFields {
    host: string;
    port: number | null;
    database: string;
    user: string;
    tls: TlsMode;
}

export type PostgresTarget = ServerFields & { engine: "postgres" };
export type MysqlTarget = ServerFields & { engine: "mysql" };

export interface SqliteTarget {
    engine: "sqlite";
    path: string;
}

export type Target = PostgresTarget | MysqlTarget | SqliteTarget;
export type ServerTarget = PostgresTarget | MysqlTarget;
export type Engine = Target["engine"];

export type DatabaseProfile = Target & {
    id: string;
    name: string;
    readOnly: boolean;
    /** Agents may run statements that change data; otherwise their connection is read-only. */
    agentWrites: boolean;
    hasPassword: boolean;
};

/** A profile as the form holds it: no id until it is first saved. */
export type ProfileDraft = Target & {
    id?: string;
    name: string;
    readOnly: boolean;
    agentWrites: boolean;
};

export interface Tested {
    version: string;
    millis: number;
}

export interface Connected {
    id: string;
    version: string;
}

export type TableKind = "table" | "view" | "materialized-view" | "foreign-table";

export interface DatabaseTable {
    name: string;
    kind: TableKind;
}

export interface ColumnInfo {
    name: string;
    type: string;
    nullable: boolean;
    default: string | null;
    primaryKey: boolean;
}

export interface IndexInfo {
    name: string;
    columns: string[];
    unique: boolean;
    primary: boolean;
}

export interface ForeignKeyInfo {
    name: string | null;
    columns: string[];
    referencesSchema: string | null;
    referencesTable: string;
    referencesColumns: string[];
}

export interface TableInfo {
    schema: string;
    name: string;
    kind: TableKind;
    columns: ColumnInfo[];
    indexes: IndexInfo[];
    foreignKeys: ForeignKeyInfo[];
}

export interface ResultColumn {
    name: string;
    /** The engine's own name for the type; empty when it does not say. */
    type: string;
    numeric: boolean;
}

/** A cell: null, a boolean, a number, or text. Huge integers and decimals arrive as text so no digit is lost. */
export type Cell = string | number | boolean | null;

export interface ResultSet {
    columns: ResultColumn[];
    rows: Cell[][];
    truncated: boolean;
    /** Rows a change touched; null for statements that return rows. */
    affected: number | null;
}

export interface QueryOutcome {
    results: ResultSet[];
    millis: number;
}

export interface HistoryEntry {
    sql: string;
    /** Milliseconds since 1970. */
    at: number;
    millis: number;
    ok: boolean;
    rows: number | null;
    error: string | null;
    source: "person" | "agent";
}

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}

export const refreshDatabase = () => invalidate((kind) => kind.startsWith("database."));

export const databaseApi = {
    profiles: () => backend.call<DatabaseProfile[]>("profiles"),
    /** A password left out keeps the saved one; an empty one forgets it. */
    save: (profile: ProfileDraft, password?: string) => backend.call<DatabaseProfile>("save", { profile, password }),
    remove: (id: string) => backend.call<void>("remove", { id }),
    test: (profile: ProfileDraft, password?: string) => backend.call<Tested>("test", { profile, password }),
    connect: (id: string) => backend.call<Connected>("connect", { id }),
    disconnect: (id: string) => backend.call<void>("disconnect", { id }),
    connected: () => backend.call<Connected[]>("connected"),
    schemas: (id: string) => backend.call<string[]>("schemas", { id }),
    tables: (id: string, schema?: string) => backend.call<DatabaseTable[]>("tables", { id, schema }),
    describe: (id: string, table: string, schema?: string) => backend.call<TableInfo>("describe", { id, table, schema }),
    /** `run` names this run, so stopping it never stops another console's query on the same connection. */
    query: (id: string, sql: string, limit?: number, run?: string) => backend.call<QueryOutcome>("query", { id, sql, limit, run }),
    cancel: (id: string, run: string) => backend.call<void>("cancel", { id, run }),
    history: (id: string, search?: string) => backend.call<HistoryEntry[]>("history", { id, search }),
    clearHistory: (id: string) => backend.call<void>("clearHistory", { id }),
};
