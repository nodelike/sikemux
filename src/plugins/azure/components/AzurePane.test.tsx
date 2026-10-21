import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
    ACCOUNT,
    FAILED_EVENT,
    GROUPS,
    INVENTORY,
    noCli,
    OK_EVENT,
    RUNNING,
    signedIn,
    signedOut,
    STOPPED,
    WEB_APP,
    WEB_APP_DETAIL,
} from "../testing";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    useSubscription: vi.fn(),
    groups: vi.fn(),
    resources: vi.fn(),
    vms: vi.fn(),
    vm: vi.fn(),
    vmAct: vi.fn(),
    apps: vi.fn(),
    app: vi.fn(),
    appAct: vi.fn(),
    tailAppLogs: vi.fn(),
    storage: vi.fn(),
    containers: vi.fn(),
    vaults: vi.fn(),
    activity: vi.fn(),
    metrics: vi.fn(),
}));
const host = vi.hoisted(() => ({ notify: vi.fn(), confirmDialog: vi.fn(), copyWithNotice: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), azureApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { azure } from "../resources";
import { AzurePane } from "./AzurePane";

let pane = 0;
const renderPane = () => render(<AzurePane paneId={`azure-${++pane}`} active />);

afterEach(() => {
    cleanup();
    azure.refresh();
});
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(signedIn);
    api.groups.mockResolvedValue(GROUPS);
    api.resources.mockResolvedValue(INVENTORY);
    api.vms.mockResolvedValue([RUNNING, STOPPED]);
    api.vm.mockResolvedValue(RUNNING);
    api.vmAct.mockResolvedValue({ message: "staging-box is starting" });
    api.apps.mockResolvedValue([WEB_APP]);
    api.app.mockResolvedValue(WEB_APP_DETAIL);
    api.storage.mockResolvedValue([ACCOUNT]);
    api.containers.mockResolvedValue([{ name: "product-images", publicAccess: "Blob", lastChanged: null, leased: false }]);
    api.vaults.mockResolvedValue([]);
    api.activity.mockResolvedValue([OK_EVENT, FAILED_EVENT]);
    api.useSubscription.mockResolvedValue(signedIn);
    host.confirmDialog.mockResolvedValue(true);
    api.tailAppLogs.mockReturnValue({ stop: vi.fn() });
});

const section = async (name: string) =>
    fireEvent.click(within(await screen.findByRole("navigation", { name: "Azure" })).getByRole("button", { name }));

describe("the Azure pane", () => {
    it("says how to install the CLI when it isn't there", async () => {
        api.status.mockResolvedValue(noCli);
        renderPane();
        expect(await screen.findByText("The Azure CLI isn't installed")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Copy the install command" }));
        expect(host.copyWithNotice).toHaveBeenCalledWith("brew install azure-cli", "The install command");
    });

    it("signs in through the CLI's browser sign-in", async () => {
        api.status.mockResolvedValue(signedOut);
        api.signIn.mockResolvedValue(signedIn);
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: "Sign in with the browser" }));
        await waitFor(() => expect(api.signIn).toHaveBeenCalled());
    });

    it("lists everything in the subscription and narrows to a resource group", async () => {
        renderPane();
        const table = await screen.findByRole("table", { name: "Resources" });
        expect(within(table).getAllByRole("row")).toHaveLength(4);
        expect(api.resources).toHaveBeenCalledWith({ subscription: "sub-1", group: undefined }, "");
        fireEvent.click(within(table).getAllByRole("button", { name: "acme-prod" })[0]);
        await waitFor(() => expect(api.resources).toHaveBeenLastCalledWith({ subscription: "sub-1", group: "acme-prod" }, ""));
    });

    it("searches for a resource by name", async () => {
        renderPane();
        await screen.findByRole("table", { name: "Resources" });
        fireEvent.change(screen.getByRole("textbox", { name: "Find a resource" }), { target: { value: " worker " } });
        fireEvent.keyDown(screen.getByRole("textbox", { name: "Find a resource" }), { key: "Enter" });
        await waitFor(() => expect(api.resources).toHaveBeenLastCalledWith({ subscription: "sub-1", group: undefined }, "worker"));
    });

    it("starts a stopped machine, and asks first before deallocating a running one", async () => {
        renderPane();
        await section("Virtual machines");
        const table = await screen.findByRole("table", { name: "Virtual machines" });
        fireEvent.click(within(table).getByRole("button", { name: "Start staging-box" }));
        await waitFor(() => expect(api.vmAct).toHaveBeenCalledWith({ subscription: "sub-1", group: "acme-staging" }, "staging-box", "start"));
        expect(host.confirmDialog).not.toHaveBeenCalled();
        fireEvent.click(within(table).getByRole("button", { name: "Stop and deallocate orders-worker-1" }));
        await waitFor(() => expect(api.vmAct).toHaveBeenLastCalledWith(expect.anything(), "orders-worker-1", "deallocate"));
        expect(host.confirmDialog.mock.calls[0][0]).toMatchObject({ destructive: true });
    });

    it("opens a machine with its addresses and an SSH button", async () => {
        renderPane();
        await section("Virtual machines");
        fireEvent.click(await screen.findByRole("row", { name: "orders-worker-1" }));
        const detail = await screen.findByLabelText("Virtual machine orders-worker-1");
        expect(within(detail).getByText("10.0.1.4")).toBeInTheDocument();
        expect(within(detail).getByRole("button", { name: "Open SSH" })).toBeInTheDocument();
        expect(api.vm).toHaveBeenCalledWith({ subscription: "sub-1", group: "acme-prod" }, "orders-worker-1");
    });

    it("shows an app's settings by name only, and follows its logs", async () => {
        renderPane();
        await section("Apps");
        fireEvent.click(await screen.findByRole("row", { name: "acme-storefront" }));
        fireEvent.click(await screen.findByRole("button", { name: /Settings/ }));
        const settings = await screen.findByRole("list", { name: "App settings" });
        expect(within(settings).getByText("DATABASE_URL")).toBeInTheDocument();
        expect(within(settings).getByText("from Key Vault")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Logs" }));
        await waitFor(() =>
            expect(api.tailAppLogs).toHaveBeenCalledWith({ subscription: "sub-1", group: "acme-prod" }, "acme-storefront", expect.anything()),
        );
    });

    it("flags a storage account whose blobs can be read without signing in", async () => {
        renderPane();
        await section("Storage");
        const card = await screen.findByRole("listitem", { name: "acmeprodassets" });
        expect(within(card).getByText("Public blobs allowed")).toBeInTheDocument();
        fireEvent.click(within(card).getByRole("button", { name: "Show containers" }));
        await waitFor(() => expect(api.containers).toHaveBeenCalledWith({ subscription: "sub-1", group: "acme-prod" }, "acmeprodassets"));
        expect(await within(card).findByText("Blob is public")).toBeInTheDocument();
    });

    it("shows what happened lately, and only what failed when asked", async () => {
        renderPane();
        await section("Activity");
        const events = await screen.findByRole("list", { name: "Activity log" });
        expect(within(events).getAllByRole("listitem")).toHaveLength(2);
        expect(within(events).getByText(/exceeding approved quota/)).toBeInTheDocument();
        fireEvent.click(within(screen.getByRole("group", { name: "Show" })).getByRole("button", { name: "Only what failed" }));
        await waitFor(() => expect(api.activity).toHaveBeenLastCalledWith({ subscription: "sub-1", group: undefined }, 24, true));
        fireEvent.click(within(screen.getByRole("group", { name: "Since" })).getByRole("button", { name: "7d" }));
        await waitFor(() => expect(api.activity).toHaveBeenLastCalledWith(expect.anything(), 168, true));
    });

    it("switches subscription and tells the CLI about it", async () => {
        renderPane();
        await screen.findByRole("table", { name: "Resources" });
        fireEvent.click(screen.getByRole("button", { name: /Subscription/ }));
        fireEvent.click(await screen.findByRole("option", { name: /Acme sandbox/ }));
        await waitFor(() => expect(api.useSubscription).toHaveBeenCalledWith("sub-2"));
    });
});
