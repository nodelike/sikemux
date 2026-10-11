import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProfile } from "../api";

const api = vi.hoisted(() => ({
    schemas: vi.fn(),
    tables: vi.fn(),
    describe: vi.fn(),
    query: vi.fn(),
    history: vi.fn(),
    profiles: vi.fn(),
    connected: vi.fn(),
    connect: vi.fn(),
    disconnect: vi.fn(),
    save: vi.fn(),
    test: vi.fn(),
    remove: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

import { invalidate } from "../../../plugin-api/resources";
import { DatabasePane } from "./DatabasePane";

const shop: DatabaseProfile = {
    id: "p1",
    name: "Shop",
    readOnly: true,
    agentWrites: false,
    hasPassword: true,
    engine: "postgres",
    host: "db.internal",
    port: null,
    database: "shop",
    user: "app",
    tls: "prefer",
};

const local: DatabaseProfile = {
    id: "p2",
    name: "Local",
    readOnly: false,
    agentWrites: false,
    hasPassword: false,
    engine: "sqlite",
    path: "/Users/me/app.db",
};

let pane = 0;
const renderPane = () => render(<DatabasePane paneId={`pane-${++pane}`} active />);

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("database."));
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.profiles.mockResolvedValue([shop, local]);
    api.connected.mockResolvedValue([]);
    api.schemas.mockResolvedValue(["public"]);
    api.tables.mockResolvedValue([{ name: "orders", kind: "table" }]);
    api.describe.mockResolvedValue({ schema: "public", name: "orders", kind: "table", columns: [], indexes: [], foreignKeys: [] });
});

describe("DatabasePane", () => {
    it("connects from the explorer, opens a console, and opens tables as tabs of their own", async () => {
        api.connect.mockResolvedValue({ id: "p1", version: "PostgreSQL 16.4" });
        api.tables.mockResolvedValue([
            { name: "customers", kind: "table" },
            { name: "orders", kind: "table" },
        ]);
        renderPane();
        api.connected.mockResolvedValue([{ id: "p1", version: "PostgreSQL 16.4" }]);
        await act(async () => fireEvent.click(await screen.findByRole("button", { name: /Shop/ })));
        expect(api.connect).toHaveBeenCalledWith("p1");
        expect(await screen.findByRole("tab", { name: "Shop console" })).toHaveAttribute("aria-selected", "true");
        expect(await screen.findByRole("region", { name: "Query Shop" })).toBeInTheDocument();
        expect(await screen.findByLabelText("Connected")).toBeInTheDocument();

        fireEvent.click(await screen.findByRole("button", { name: /^orders/ }));
        expect(await screen.findByRole("article", { name: "public.orders" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: /^customers/ }));
        expect(screen.getByRole("tab", { name: "Shop customers" })).toHaveAttribute("aria-selected", "true");
        expect(screen.getAllByRole("tab").map((tab) => tab.getAttribute("aria-label"))).toEqual(["Shop console", "Shop orders", "Shop customers"]);
    });

    it("keeps tabs from different connections side by side", async () => {
        api.connected.mockResolvedValue([
            { id: "p1", version: "PostgreSQL 16.4" },
            { id: "p2", version: "SQLite 3.46.0" },
        ]);
        api.history.mockResolvedValue([]);
        renderPane();
        fireEvent.contextMenu(await screen.findByRole("button", { name: /Shop/ }));
        fireEvent.click(screen.getByText("New console"));
        fireEvent.contextMenu(screen.getByRole("button", { name: /Local/ }));
        fireEvent.click(screen.getByText("History"));
        expect(screen.getAllByRole("tab").map((tab) => tab.getAttribute("aria-label"))).toEqual(["Shop console", "Local history"]);
        expect(await screen.findByRole("region", { name: "History of Local" })).toBeInTheDocument();

        fireEvent.click(screen.getByRole("tab", { name: "Shop console" }));
        expect(screen.getByRole("region", { name: "Query Shop" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Close Shop console" }));
        expect(screen.getByRole("tab", { name: "Local history" })).toHaveAttribute("aria-selected", "true");
    });

    it("previews a table into its connection's console", async () => {
        api.connected.mockResolvedValue([{ id: "p1", version: "PostgreSQL 16.4" }]);
        api.query.mockResolvedValue({ results: [], millis: 2 });
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /Shop/ }));
        fireEvent.contextMenu(await screen.findByRole("button", { name: /public/ }));
        fireEvent.click(screen.getByText("Refresh"));
        fireEvent.click(screen.getByRole("button", { name: /public/ }));
        fireEvent.click(await screen.findByRole("button", { name: /^tables/ }));
        await act(async () => fireEvent.doubleClick(await screen.findByRole("button", { name: /^orders/ })));
        expect(api.query).toHaveBeenCalledWith("p1", 'select * from "public"."orders" limit 100;', 500, expect.any(String));
        expect(screen.getByRole("tab", { name: "Shop console" })).toHaveAttribute("aria-selected", "true");
    });
});
