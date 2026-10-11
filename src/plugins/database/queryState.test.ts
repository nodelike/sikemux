import { afterEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ query: vi.fn(), cancel: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

import { forgetQuery, loadQuery, readQuery, runQuery, stopQuery, updateQuery } from "./queryState";

const outcome = { results: [{ columns: [], rows: [], truncated: false, affected: 1 }], millis: 4 };

afterEach(() => {
    forgetQuery("c1");
    api.query.mockReset();
    api.cancel.mockReset();
});

describe("queryState", () => {
    it("runs the SQL with the chosen row limit and keeps what came back", async () => {
        api.query.mockResolvedValue(outcome);
        updateQuery("c1", { limit: 100 });
        await runQuery("c1", "p1", "  update t set a = 1  ");
        expect(api.query).toHaveBeenCalledWith("p1", "update t set a = 1", 100, expect.any(String));
        expect(readQuery("c1")).toMatchObject({ outcome, error: null, running: false, ran: "update t set a = 1", run: null });
    });

    it("keeps each console's SQL and results apart on one connection", async () => {
        api.query.mockResolvedValue(outcome);
        updateQuery("c2", { sql: "select 2" });
        await runQuery("c1", "p1", "select 1");
        expect(readQuery("c1").ran).toBe("select 1");
        expect(readQuery("c2")).toMatchObject({ sql: "select 2", ran: null, outcome: null });
        forgetQuery("c2");
    });

    it("keeps the database's own error and clears the old result", async () => {
        api.query.mockResolvedValueOnce(outcome).mockRejectedValueOnce({ category: "query", message: 'relation "t" does not exist' });
        await runQuery("c1", "p1", "select 1");
        await runQuery("c1", "p1", "select * from t");
        expect(readQuery("c1")).toMatchObject({ outcome: null, error: 'relation "t" does not exist', running: false });
    });

    it("does not start a second run while one is going, nor run empty SQL", async () => {
        let finish: (value: unknown) => void = () => {};
        api.query.mockReturnValue(new Promise((resolve) => (finish = resolve)));
        const first = runQuery("c1", "p1", "select pg_sleep(10)");
        await runQuery("c1", "p1", "select 2");
        await runQuery("c1", "p1", "   ");
        expect(api.query).toHaveBeenCalledTimes(1);
        expect(readQuery("c1").running).toBe(true);
        finish(outcome);
        await first;
        expect(readQuery("c1").running).toBe(false);
    });

    it("stops only its own run and loads SQL without running it", async () => {
        let finish: (value: unknown) => void = () => {};
        api.query.mockReturnValue(new Promise((resolve) => (finish = resolve)));
        api.cancel.mockResolvedValue(undefined);
        const running = runQuery("c1", "p1", "select pg_sleep(10)");
        const run = api.query.mock.calls[0][3] as string;
        await stopQuery("c2", "p1");
        expect(api.cancel).not.toHaveBeenCalled();
        await stopQuery("c1", "p1");
        expect(api.cancel).toHaveBeenCalledWith("p1", run);
        finish(outcome);
        await running;
        api.query.mockReset();
        loadQuery("c1", "select * from orders limit 100;");
        expect(readQuery("c1").sql).toBe("select * from orders limit 100;");
        expect(api.query).not.toHaveBeenCalled();
    });
});
