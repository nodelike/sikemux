import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenNodes } from "../resources";
import type { DatabaseConnection, DatabaseExplorer, DatabaseTableEntry, DatabaseTableKind } from "../types.generated";

const api = vi.hoisted(() => ({
    query: vi.fn(),
    profiles: vi.fn(),
    connect: vi.fn(),
    disconnect: vi.fn(),
    save: vi.fn(),
    test: vi.fn(),
    remove: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

const views = vi.hoisted(() => ({
    explorerView: vi.fn(),
    tableView: vi.fn(),
    historyView: vi.fn(),
}));

import { fakeViews } from "../../../plugin-api/fakeViews";
import { database } from "../resources";
import { DatabasePane } from "./DatabasePane";

fakeViews(views);

const saved = (id: string, name: string, engine: DatabaseConnection["engine"], defaultSchema: string): DatabaseConnection => ({
    id,
    name,
    engine,
    engineName: engine,
    address: name.toLowerCase(),
    readOnly: false,
    connected: null,
    defaultSchema,
    facts: [],
    schemas: null,
    failed: null,
});

const shop = saved("p1", "Shop", "postgres", "public");
const local = saved("p2", "Local", "sqlite", "main");

const tables: Record<string, [string, DatabaseTableKind][]> = {
    audit: [["events", "table"]],
    public: [
        ["customers", "table"],
        ["orders", "table"],
        ["big_orders", "view"],
    ],
};

/** The explorer the plugin would serve for what is open, with these connections connected. */
function explorerOf(connected: Record<string, string>) {
    return ({ open }: { open: OpenNodes[] }): DatabaseExplorer => ({
        connections: [shop, local].map((connection) => {
            const version = connected[connection.id] ?? null;
            const opened = open.find((each) => each.id === connection.id);
            if (!version || !opened) return { ...connection, connected: version };
            const schemas = Object.keys(tables).map((schema) => {
                if (!opened.schemas.includes(schema)) return { name: schema, groups: null, failed: null };
                const entries: DatabaseTableEntry[] = tables[schema].map(([name, kind]) => ({
                    name,
                    kind,
                    kindName: kind,
                    preview: `select * from "${schema}"."${name}" limit 100;`,
                    columns: null,
                    failed: null,
                }));
                const groups = [
                    { name: "tables", tables: entries.filter((entry) => entry.kind === "table") },
                    { name: "views", tables: entries.filter((entry) => entry.kind !== "table") },
                ];
                return { name: schema, groups, failed: null };
            });
            return { ...connection, connected: version, schemas };
        }),
    });
}

let pane = 0;
const renderPane = () => render(<DatabasePane paneId={`pane-${++pane}`} active />);

afterEach(() => {
    cleanup();
    database.refresh();
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(views).forEach((mock) => mock.mockReset());
    views.explorerView.mockImplementation(explorerOf({}));
    views.tableView.mockResolvedValue({
        schema: "public",
        name: "orders",
        kind: "table",
        kindName: "table",
        preview: 'select * from "public"."orders" limit 100;',
        columns: [],
        indexes: [],
        foreignKeys: [],
    });
    views.historyView.mockResolvedValue({ entries: [], empty: "Queries you and your agents run here show up in this list." });
});

describe("DatabasePane", () => {
    it("connects from the explorer, opens a console, and opens tables as tabs of their own", async () => {
        api.connect.mockImplementation(async () => {
            views.explorerView.mockImplementation(explorerOf({ p1: "PostgreSQL 16.4" }));
        });
        renderPane();
        await act(async () => fireEvent.click(await screen.findByRole("button", { name: /Shop/ })));
        expect(api.connect).toHaveBeenCalledWith("p1");
        expect(await screen.findByRole("tab", { name: "Shop console" })).toHaveAttribute("aria-selected", "true");
        expect(await screen.findByRole("region", { name: "Query Shop" })).toBeInTheDocument();
        expect(await screen.findByLabelText("Connected")).toBeInTheDocument();

        const shopNode = screen.getByRole("treeitem", { name: "Shop" });
        expect(await within(shopNode).findByRole("treeitem", { name: "orders" })).toBeInTheDocument();
        expect(within(shopNode).getByRole("treeitem", { name: "audit" })).toHaveAttribute("aria-expanded", "false");
        expect(within(shopNode).getByRole("treeitem", { name: "public tables" })).toHaveTextContent("2");
        expect(within(shopNode).getByRole("treeitem", { name: "public views" })).toHaveAttribute("aria-expanded", "false");

        fireEvent.click(screen.getByRole("button", { name: /^orders/ }));
        expect(await screen.findByRole("article", { name: "public.orders" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: /^customers/ }));
        expect(screen.getByRole("tab", { name: "Shop customers" })).toHaveAttribute("aria-selected", "true");
        expect(screen.getAllByRole("tab").map((tab) => tab.getAttribute("aria-label"))).toEqual(["Shop console", "Shop orders", "Shop customers"]);
    });

    it("narrows tables to the filter and says how many match", async () => {
        api.connect.mockImplementation(async () => {
            views.explorerView.mockImplementation(explorerOf({ p1: "PostgreSQL 16.4" }));
        });
        renderPane();
        await act(async () => fireEvent.click(await screen.findByRole("button", { name: /Shop/ })));
        fireEvent.change(screen.getByRole("textbox", { name: "Filter tables" }), { target: { value: "ord" } });
        const group = await screen.findByRole("treeitem", { name: "public tables" });
        expect(await within(group).findByRole("treeitem", { name: "orders" })).toBeInTheDocument();
        expect(within(group).queryByRole("treeitem", { name: "customers" })).toBeNull();
        expect(group).toHaveTextContent("1 of 2");
    });

    it("keeps tabs from different connections side by side", async () => {
        views.explorerView.mockImplementation(explorerOf({ p1: "PostgreSQL 16.4", p2: "SQLite 3.46.0" }));
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
        views.explorerView.mockImplementation(explorerOf({ p1: "PostgreSQL 16.4" }));
        api.query.mockResolvedValue({ results: [], millis: 2 });
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /Shop/ }));
        fireEvent.click(await screen.findByRole("button", { name: /public/ }));
        fireEvent.click(await screen.findByRole("button", { name: /^tables/ }));
        await act(async () => fireEvent.doubleClick(await screen.findByRole("button", { name: /^orders/ })));
        expect(api.query).toHaveBeenCalledWith("p1", 'select * from "public"."orders" limit 100;', 500, expect.any(String));
        expect(screen.getByRole("tab", { name: "Shop console" })).toHaveAttribute("aria-selected", "true");
    });
});
