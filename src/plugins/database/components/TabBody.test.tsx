import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProfile } from "../api";

const api = vi.hoisted(() => ({ describe: vi.fn(), history: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

import { invalidate } from "../../../plugin-api/resources";
import { historyTab, tableTab, type DatabaseTab } from "../tabs";
import { TabBody } from "./TabBody";

const shop: DatabaseProfile = { id: "p1", name: "Shop", readOnly: false, agentWrites: false, hasPassword: false, engine: "sqlite", path: "/a.db" };
const connected = { id: "p1", version: "SQLite 3.46.0" };

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("database."));
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.history.mockResolvedValue([]);
});

function renderTab(tab: DatabaseTab, open = true) {
    const props = { onQuery: vi.fn(), onOpenTable: vi.fn(), onEdit: vi.fn() };
    render(<TabBody tab={tab} profile={shop} connected={open ? connected : null} active {...props} />);
    return props;
}

describe("TabBody", () => {
    it("offers to connect before showing a console or table on a closed connection", () => {
        renderTab(tableTab("p1", "main", "orders"), false);
        expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
        expect(api.describe).not.toHaveBeenCalled();
    });

    it("shows the history whether or not the connection is open", async () => {
        renderTab(historyTab("p1"), false);
        expect(await screen.findByRole("region", { name: "History of Shop" })).toBeInTheDocument();
    });
});
