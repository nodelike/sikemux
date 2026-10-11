import { create } from "zustand";
import { invalidate } from "../../plugin-api/resources";
import { databaseApi, failureMessage, type QueryOutcome } from "./api";

export const ROW_LIMITS = [100, 500, 1000, 5000] as const;
export const DEFAULT_ROW_LIMIT = 500;

/** One console's query: the SQL being written, and what the last run gave. */
export interface QueryState {
    sql: string;
    limit: number;
    running: boolean;
    outcome: QueryOutcome | null;
    error: string | null;
    /** The SQL of the last run, shown beside its outcome. */
    ran: string | null;
    /** Names the run going now, so Stop reaches it and not another console's. */
    run: string | null;
}

const EMPTY: QueryState = { sql: "", limit: DEFAULT_ROW_LIMIT, running: false, outcome: null, error: null, ran: null, run: null };

const useQueries = create<{ queries: Record<string, QueryState> }>(() => ({ queries: {} }));

export const useQuery = (id: string): QueryState => useQueries((state) => state.queries[id] ?? EMPTY);

export const readQuery = (id: string): QueryState => useQueries.getState().queries[id] ?? EMPTY;

export function updateQuery(id: string, change: Partial<QueryState>): void {
    useQueries.setState((state) => ({ queries: { ...state.queries, [id]: { ...(state.queries[id] ?? EMPTY), ...change } } }));
}

/** Runs the console's SQL on a saved database. A run already going is left alone rather than queued. */
export async function runQuery(id: string, profile: string, sql: string): Promise<void> {
    const text = sql.trim();
    if (!text || readQuery(id).running) return;
    const run = crypto.randomUUID();
    updateQuery(id, { running: true, error: null, ran: text, run });
    try {
        const outcome = await databaseApi.query(profile, text, readQuery(id).limit, run);
        updateQuery(id, { outcome, error: null });
    } catch (failure) {
        updateQuery(id, { outcome: null, error: failureMessage(failure) });
    } finally {
        updateQuery(id, { running: false, run: null });
        invalidate((kind) => kind === "database.history" || kind === "database.connected");
    }
}

/** Stops the console's own run, leaving other consoles' queries on the same connection alone. */
export async function stopQuery(id: string, profile: string): Promise<void> {
    const { run } = readQuery(id);
    if (run) await databaseApi.cancel(profile, run);
}

/** Puts SQL in the editor without running it, as when a table or a past query is picked. */
export function loadQuery(id: string, sql: string): void {
    updateQuery(id, { sql });
}

/** Forgets a console's query, as when its tab closes. */
export function forgetQuery(id: string): void {
    useQueries.setState((state) => {
        const queries = { ...state.queries };
        delete queries[id];
        return { queries };
    });
}
