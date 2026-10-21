import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});

import { answerEmpty } from "../testing";
import { BigQueryPage } from "./BigQueryPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const schema = [
    { name: "order_id", type: "INTEGER", mode: "REQUIRED", description: null, fields: [] },
    {
        name: "address",
        type: "RECORD",
        mode: "NULLABLE",
        description: null,
        fields: [{ name: "city", type: "STRING", mode: "NULLABLE", description: null, fields: [] }],
    },
];

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.datasets.mockResolvedValue([{ id: "shop", location: "US", name: null }]);
    api.tables.mockResolvedValue([{ id: "orders", type: "TABLE", partitioned: true }]);
    api.table.mockResolvedValue({
        id: "orders",
        type: "TABLE",
        description: null,
        rows: 18234,
        bytes: 1048576,
        location: "US",
        modifiedAt: null,
        partitionedBy: "created_at",
        clusteredBy: ["customer_id"],
        view: null,
        schema,
        preview: [{ order_id: "1", address: { city: "Lisbon" } }],
    });
    api.query.mockImplementation((_target, _sql: string, dryRun: boolean) =>
        Promise.resolve(
            dryRun
                ? { dryRun: true, bytes: 1048576, costUsd: 0.0001, schema }
                : {
                      dryRun: false,
                      schema,
                      rows: [{ order_id: "1", address: { city: "Lisbon" } }],
                      totalRows: 1,
                      bytes: 10485760,
                      costUsd: 0.0001,
                      cached: false,
                      job: "job_1",
                  },
        ),
    );
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("BigQuery", () => {
    it("opens a table from the explorer to its nested schema and starts a query on it", async () => {
        render(<BigQueryPage paneId="bq-1" active target={target} />);
        fireEvent.click(await screen.findByRole("button", { name: /shop/ }));
        fireEvent.click(await screen.findByRole("button", { name: /orders/ }));
        expect(await screen.findByText("city")).toBeInTheDocument();
        expect((screen.getByRole("textbox", { name: "SQL" }) as HTMLTextAreaElement).value).toContain("FROM `acme-prod.shop.orders`");
        fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
        expect(await screen.findByText('{"city":"Lisbon"}')).toBeInTheDocument();
    });

    it("estimates a query's cost, then runs it", async () => {
        render(<BigQueryPage paneId="bq-2" active target={target} />);
        fireEvent.change(screen.getByRole("textbox", { name: "SQL" }), { target: { value: "SELECT 1" } });
        fireEvent.click(screen.getByRole("button", { name: "Estimate" }));
        expect(await screen.findByText("1.0 MiB")).toBeInTheDocument();
        expect(api.query).toHaveBeenCalledWith(target, "SELECT 1", true);
        fireEvent.click(screen.getByRole("button", { name: "Run" }));
        await waitFor(() => expect(api.query).toHaveBeenLastCalledWith(target, "SELECT 1", false));
        expect(await screen.findByText(/1 rows · 10\.0 MiB billed/)).toBeInTheDocument();
    });
});
