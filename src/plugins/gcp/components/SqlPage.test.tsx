import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const host = vi.hoisted(() => ({ confirm: vi.fn(() => Promise.resolve(true)), terminal: vi.fn(() => Promise.resolve()) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: host.confirm,
    runInTerminal: host.terminal,
}));

import { answerEmpty } from "../testing";
import { changesFor, SqlPage } from "./SqlPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const instance = {
    name: "orders",
    engine: "POSTGRES_16",
    region: "us-central1",
    zone: "us-central1-a",
    state: "RUNNING",
    tier: "db-custom-2-7680",
    diskGb: 100,
    highAvailability: true,
    backups: true,
    backupWindow: "03:00",
    publicIp: "34.9.9.9",
    privateIp: null,
    connectionName: "acme-prod:us-central1:orders",
    createdAt: null,
};

let pane = 0;
const renderPage = () => render(<SqlPage paneId={`sql-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.sqlInstances.mockResolvedValue([instance]);
    api.sqlInstance.mockResolvedValue({
        ...instance,
        databases: [{ name: "orders", charset: "UTF8" }],
        users: [{ name: "postgres", host: null, type: null }],
        recentBackups: [],
        connect: ["gcloud", "sql", "connect", "orders", "--project=acme-prod"],
    });
    api.sqlChange.mockResolvedValue({ operation: "op" });
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Cloud SQL", () => {
    it("offers only the changes an instance's state allows", () => {
        expect(changesFor("RUNNING")).toEqual(["backup", "restart", "stop"]);
        expect(changesFor("STOPPED")).toEqual(["start"]);
        expect(changesFor("MAINTENANCE")).toEqual([]);
    });

    it("lists instances by engine and address, and opens one to connect and back up", async () => {
        renderPage();
        expect(await screen.findByText("PostgreSQL 16")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("row", { name: /orders/ }));
        expect(await screen.findByText("acme-prod:us-central1:orders")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Connect" }));
        expect(host.terminal).toHaveBeenCalledWith("Connect to orders", ["gcloud", "sql", "connect", "orders", "--project=acme-prod"]);
        fireEvent.click(screen.getByRole("button", { name: "Back up now" }));
        await waitFor(() => expect(api.sqlChange).toHaveBeenCalledWith(target, "orders", "backup"));
    });
});
