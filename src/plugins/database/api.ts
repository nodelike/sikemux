import { createPluginBackend } from "../../plugin-api/backend";
import { DATABASE_PLUGIN } from "./kinds";
import type { DatabaseCopyFormat, DatabaseProfileDraft, DatabaseProfileForm, DatabaseQueryOutcome, DatabaseResultSet } from "./types.generated";

const backend = createPluginBackend(DATABASE_PLUGIN.id);

export interface Tested {
    version: string;
    millis: number;
}

export const databaseApi = {
    /** The form for the saved connection with this id, or for a new one. */
    form: (id?: string) => backend.call<DatabaseProfileForm>("form", { id }),
    /** A password left out keeps the saved one; an empty one forgets it. */
    save: (profile: DatabaseProfileDraft, password?: string) => backend.call<void>("save", { profile, password }),
    remove: (id: string) => backend.call<void>("remove", { id }),
    test: (profile: DatabaseProfileDraft, password?: string) => backend.call<Tested>("test", { profile, password }),
    connect: (id: string) => backend.call<void>("connect", { id }),
    disconnect: (id: string) => backend.call<void>("disconnect", { id }),
    /** `run` names this run, so stopping it never stops another console's query on the same connection. */
    query: (id: string, sql: string, limit?: number, run?: string) => backend.call<DatabaseQueryOutcome>("query", { id, sql, limit, run }),
    /** The result as text to paste elsewhere, with a header row. */
    copyResult: (result: DatabaseResultSet, format: DatabaseCopyFormat) =>
        backend.call<string>("copyResult", { result: { columns: result.columns, rows: result.rows }, format }),
    cancel: (id: string, run: string) => backend.call<void>("cancel", { id, run }),
    clearHistory: (id: string) => backend.call<void>("clearHistory", { id }),
};
