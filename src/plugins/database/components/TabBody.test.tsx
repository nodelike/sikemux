import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseConnection } from "../types.generated";

const views = vi.hoisted(() => ({ tableView: vi.fn(), historyView: vi.fn() }));

import { fakeViews } from "../../../plugin-api/fakeViews";
import { historyTab, tableTab, type DatabaseTab } from "../tabs";
import { TabBody } from "./TabBody";
import { database } from "../resources";

fakeViews(views);

const shop: DatabaseConnection = {
    id: "p1",
    name: "Shop",
    engine: "sqlite",
    engineName: "SQLite",
    address: "a.db",
    readOnly: false,
    connected: "SQLite 3.46.0",
    defaultSchema: "main",
    facts: [],
    schemas: null,
    failed: null,
};

afterEach(() => {
    cleanup();
    database.refresh();
});
beforeEach(() => {
    Object.values(views).forEach((mock) => mock.mockReset());
    views.historyView.mockResolvedValue({ entries: [], empty: "" });
});

function renderTab(tab: DatabaseTab, open = true) {
    const props = { onQuery: vi.fn(), onOpenTable: vi.fn(), onEdit: vi.fn() };
    render(<TabBody slot="pane/tab" tab={tab} profile={open ? shop : { ...shop, connected: null }} active {...props} />);
    return props;
}

describe("TabBody", () => {
    it("offers to connect before showing a console or table on a closed connection", () => {
        renderTab(tableTab("p1", "main", "orders"), false);
        expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
        expect(views.tableView).not.toHaveBeenCalled();
    });

    it("shows the history whether or not the connection is open", async () => {
        renderTab(historyTab("p1"), false);
        expect(await screen.findByRole("region", { name: "History of Shop" })).toBeInTheDocument();
    });
});
