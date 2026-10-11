import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseProfile, TableInfo } from "../api";

const api = vi.hoisted(() => ({ describe: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), databaseApi: api }));

import { invalidate } from "../../../plugin-api/resources";
import { TableView } from "./TableView";

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

const orders: TableInfo = {
    schema: "public",
    name: "orders",
    kind: "table",
    columns: [
        { name: "id", type: "bigint", nullable: false, default: "nextval('orders_id_seq'::regclass)", primaryKey: true },
        { name: "customer_id", type: "integer", nullable: false, default: null, primaryKey: false },
        { name: "note", type: "text", nullable: true, default: null, primaryKey: false },
    ],
    indexes: [
        { name: "orders_pkey", columns: ["id"], unique: true, primary: true },
        { name: "orders_by_customer", columns: ["customer_id"], unique: false, primary: false },
    ],
    foreignKeys: [
        {
            name: "orders_customer_id_fkey",
            columns: ["customer_id"],
            referencesSchema: "public",
            referencesTable: "customers",
            referencesColumns: ["id"],
        },
    ],
};

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("database."));
});
beforeEach(() => {
    api.describe.mockReset();
    api.describe.mockResolvedValue(orders);
});

const renderView = (profile: DatabaseProfile = shop) => {
    const onQuery = vi.fn();
    const onOpenTable = vi.fn();
    render(<TableView profile={profile} table={{ schema: "public", name: "orders" }} active onQuery={onQuery} onOpenTable={onOpenTable} />);
    return { onQuery, onOpenTable };
};

describe("TableView", () => {
    it("leaves SQLite's main schema out of the query", async () => {
        api.describe.mockResolvedValue({ ...orders, schema: "main" });
        const { onQuery } = renderView({
            id: "p2",
            name: "Local",
            readOnly: false,
            agentWrites: false,
            hasPassword: false,
            engine: "sqlite",
            path: "/a.db",
        });
        fireEvent.click(await screen.findByRole("button", { name: "Preview rows" }));
        expect(onQuery).toHaveBeenLastCalledWith('select * from "orders" limit 100;', true);
    });
});
