import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    runInTerminal: vi.fn(() => Promise.resolve()),
}));

import { runInTerminal } from "../../../plugin-api/host";
import { answerEmpty, signedIn } from "../testing";
import { gcpSettings } from "../state";
import { GcpPane } from "./GcpPane";
import { gcp } from "../resources";

let pane = 0;
const renderPane = () => render(<GcpPane paneId={`gcp-${++pane}`} active />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    gcpSettings.update(() => ({ account: null, project: null, sideWidth: null }));
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("the Google Cloud console", () => {
    it("offers to install gcloud when it is missing", async () => {
        api.status.mockResolvedValue({ ...signedIn, installed: false, accounts: [] });
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: "Install with Homebrew" }));
        expect(runInTerminal).toHaveBeenCalledWith("Install the Google Cloud CLI", ["brew", "install", "--cask", "gcloud-cli"]);
    });

    it("signs in through gcloud when no account is signed in", async () => {
        api.status.mockResolvedValue({ ...signedIn, accounts: [] });
        api.signIn.mockReturnValue({ stop: vi.fn() });
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: "Sign in with Google" }));
        expect(api.signIn).toHaveBeenCalled();
        expect(await screen.findByText("Finish signing in in your browser")).toBeInTheDocument();
    });

    it("opens on gcloud's active project and counts its resources", async () => {
        api.runServices.mockResolvedValue([
            { name: "api", location: "us-central1", state: "READY" },
            { name: "worker", location: "us-central1", state: "FAILED" },
        ]);
        api.instances.mockResolvedValue([
            { name: "vm-1", status: "RUNNING" },
            { name: "vm-2", status: "TERMINATED" },
        ]);
        renderPane();
        expect(await screen.findByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
        await waitFor(() => expect(gcpSettings.get().project).toBe("acme-prod"));
        expect(screen.getByRole("button", { name: /acme-prod/ })).toBeInTheDocument();
        expect(await screen.findByText("1 failing")).toBeInTheDocument();
        expect(await screen.findByText("1 running")).toBeInTheDocument();
        expect(api.runServices).toHaveBeenCalledWith({ account: null, project: "acme-prod" });
        expect(await screen.findByText("Enabled")).toBeInTheDocument();
        expect(screen.getByText("Budgets need the Billing Viewer role on the account.")).toBeInTheDocument();
    });

    it("picks another project from the top bar", async () => {
        api.projects.mockResolvedValue([
            { id: "acme-prod", name: "Acme Prod", number: "1", createdAt: "" },
            { id: "acme-staging", name: "Acme Staging", number: "2", createdAt: "" },
        ]);
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /acme-prod/ }));
        fireEvent.click(await screen.findByRole("option", { name: /Acme Staging/ }));
        await waitFor(() => expect(gcpSettings.get().project).toBe("acme-staging"));
    });
});
