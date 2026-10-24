import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseConnection, DatabaseHistoryEntry } from "../types.generated";

const api = vi.hoisted(() => ({ clearHistory: vi.fn() }));
const views = vi.hoisted(() => ({ historyView: vi.fn() }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { fakeViews } from "../../../plugin-api/fakeViews";
import { HistoryPanel } from "./HistoryPanel";
import { database } from "../resources";

fakeViews(views);

const NOW = Date.UTC(2026, 9, 4, 12, 0, 0);

const shop: DatabaseConnection = {
    id: "p1",
    name: "Shop",
    engine: "sqlite",
    engineName: "SQLite",
    address: "a.db",
    readOnly: false,
    connected: null,
    defaultSchema: "main",
    facts: [],
    schemas: null,
    failed: null,
};

const entries: DatabaseHistoryEntry[] = [
    { sql: "select * from nowhere", at: NOW - 30_000, millis: 2, ok: false, rows: null, error: "no such table: nowhere", byAgent: true },
    { sql: "select * from orders", at: NOW - 600_000, millis: 14, ok: true, rows: 2, error: null, byAgent: false },
];

afterEach(() => {
    cleanup();
    database.refresh();
});
beforeEach(() => {
    views.historyView.mockReset();
    api.clearHistory.mockReset();
    host.confirmDialog.mockReset();
    views.historyView.mockResolvedValue({ entries, empty: "" });
});

describe("HistoryPanel", () => {
    it("clears the history only once it is confirmed", async () => {
        render(<HistoryPanel slot="pane/tab" profile={shop} active onOpen={vi.fn()} now={NOW} />);
        await screen.findAllByRole("listitem");
        host.confirmDialog.mockResolvedValue(false);
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Clear history" })));
        expect(api.clearHistory).not.toHaveBeenCalled();
        host.confirmDialog.mockResolvedValue(true);
        api.clearHistory.mockResolvedValue(undefined);
        await act(async () => fireEvent.click(screen.getByRole("button", { name: "Clear history" })));
        expect(api.clearHistory).toHaveBeenCalledWith("p1");
    });
});
