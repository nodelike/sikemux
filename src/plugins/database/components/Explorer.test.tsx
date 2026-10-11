import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Connected, DatabaseProfile } from "../api";

const api = vi.hoisted(() => ({ schemas: vi.fn(), tables: vi.fn(), describe: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

import { invalidate } from "../../../plugin-api/resources";
import { Explorer, type ExplorerActions } from "./Explorer";

const shop: DatabaseProfile = {
    id: "p1",
    name: "Shop",
    readOnly: false,
    agentWrites: false,
    hasPassword: false,
    engine: "postgres",
    host: "localhost",
    port: null,
    database: "shop",
    user: "app",
    tls: "prefer",
};
const local: DatabaseProfile = { id: "p2", name: "Local", readOnly: false, agentWrites: false, hasPassword: false, engine: "sqlite", path: "/a.db" };

const actions = (): ExplorerActions => ({
    connect: vi.fn().mockResolvedValue(undefined),
    disconnect: vi.fn().mockResolvedValue(undefined),
    refresh: vi.fn(),
    newConsole: vi.fn(),
    history: vi.fn(),
    properties: vi.fn(),
    edit: vi.fn(),
    openTable: vi.fn(),
    queryTable: vi.fn(),
    copy: vi.fn(),
});

function Harness({
    initial = [],
    connected,
    filter = "",
    act: doing,
}: {
    initial?: string[];
    connected: Connected[];
    filter?: string;
    act: ExplorerActions;
}) {
    const [expanded, setExpanded] = useState(initial);
    return (
        <Explorer
            profiles={[shop, local]}
            connected={connected}
            active
            expanded={expanded}
            filter={filter}
            current={null}
            onToggle={(key) => setExpanded((now) => (now.includes(key) ? now.filter((each) => each !== key) : [...now, key]))}
            onExpand={(keys) => setExpanded((now) => [...now, ...keys.filter((key) => !now.includes(key))])}
            actions={doing}
        />
    );
}

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("database."));
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.schemas.mockResolvedValue(["audit", "public"]);
    api.tables.mockImplementation(async (_id: string, schema: string) =>
        schema === "public"
            ? [
                  { name: "customers", kind: "table" },
                  { name: "orders", kind: "table" },
                  { name: "big_orders", kind: "view" },
              ]
            : [{ name: "events", kind: "table" }],
    );
    api.describe.mockResolvedValue({
        schema: "public",
        name: "orders",
        kind: "table",
        columns: [
            { name: "id", type: "bigint", nullable: false, default: null, primaryKey: true },
            { name: "total", type: "numeric(10,2)", nullable: false, default: "0", primaryKey: false },
        ],
        indexes: [],
        foreignKeys: [],
    });
});

describe("Explorer", () => {
    it("lists every connection, and connecting opens its usual schema's tables", async () => {
        const doing = actions();
        const { rerender } = render(<Harness connected={[]} act={doing} />);
        expect(screen.getByRole("treeitem", { name: "Shop" })).toBeInTheDocument();
        expect(screen.getByRole("treeitem", { name: "Local" })).toBeInTheDocument();

        await act(async () => fireEvent.click(screen.getByRole("button", { name: /Shop/ })));
        expect(doing.connect).toHaveBeenCalledWith(shop);
        rerender(<Harness connected={[{ id: "p1", version: "PostgreSQL 16.4" }]} act={doing} />);

        const shopNode = screen.getByRole("treeitem", { name: "Shop" });
        expect(await within(shopNode).findByRole("treeitem", { name: "orders" })).toBeInTheDocument();
        expect(within(shopNode).getByRole("treeitem", { name: "audit" })).toHaveAttribute("aria-expanded", "false");
        expect(within(shopNode).getByRole("treeitem", { name: "public tables" })).toHaveTextContent("2");
        expect(within(shopNode).getByRole("treeitem", { name: "public views" })).toHaveAttribute("aria-expanded", "false");
    });

    it("narrows tables to the filter and says how many match", async () => {
        render(
            <Harness
                connected={[{ id: "p1", version: "16" }]}
                initial={["c:p1", "s:p1:public", "g:p1:public:tables"]}
                filter="ord"
                act={actions()}
            />,
        );
        const group = await screen.findByRole("treeitem", { name: "public tables" });
        expect(await within(group).findByRole("treeitem", { name: "orders" })).toBeInTheDocument();
        expect(within(group).queryByRole("treeitem", { name: "customers" })).toBeNull();
        expect(group).toHaveTextContent("1 of 2");
    });
});
