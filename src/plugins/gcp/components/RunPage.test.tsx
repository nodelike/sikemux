import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const confirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
const prompt = vi.hoisted(() => vi.fn<(request: { label?: string }) => Promise<string | null>>());
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: confirm,
    promptDialog: prompt,
}));

import { answerEmpty } from "../testing";
import { RunPage } from "./RunPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const service = {
    name: "api",
    location: "us-central1",
    url: "https://api-abc-uc.a.run.app",
    state: "READY",
    message: null,
    latestRevision: "api-00012-xyz",
    image: "us-docker.pkg.dev/acme/api:1.4",
    cpu: "1",
    memory: "512Mi",
    minInstances: 0,
    maxInstances: 10,
    concurrency: 80,
    serviceAccount: "api@acme.iam.gserviceaccount.com",
    ingress: "INGRESS_TRAFFIC_ALL",
    traffic: [
        { revision: "api-00012-xyz", latest: false, percent: 90, tag: null, url: null },
        { revision: "api-00011-abc", latest: false, percent: 10, tag: "canary", url: null },
    ],
    updatedAt: "2024-10-18T10:00:00Z",
    updatedBy: "ankit@example.com",
};
const detail = {
    ...service,
    revisions: [
        { name: "api-00012-xyz", state: "READY", image: "img:1.4", createdAt: "2024-10-18T10:00:00Z", percent: 90, tags: [] },
        { name: "api-00011-abc", state: "READY", image: "img:1.3", createdAt: "2024-10-17T10:00:00Z", percent: 10, tags: ["canary"] },
    ],
    env: [
        { name: "MODE", secret: null },
        { name: "DB_PASSWORD", secret: "db-password" },
    ],
};

let pane = 0;
const renderPage = () => render(<RunPage paneId={`run-${++pane}`} active target={target} />);

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.runServices.mockResolvedValue([service]);
    api.runService.mockResolvedValue(detail);
    api.runSetTraffic.mockResolvedValue({ operation: "op-1" });
    confirm.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Cloud Run", () => {
    it("lists services with their region, URL and traffic split", async () => {
        renderPage();
        const table = await screen.findByRole("table", { name: "Services" });
        expect(within(table).getByText("api")).toBeInTheDocument();
        expect(within(table).getByText("90% api-00012-xyz, 10% api-00011-abc")).toBeInTheDocument();
        expect(within(table).getByText("api-abc-uc.a.run.app")).toBeInTheDocument();
    });

    it("opens a service and rolls traffic back to an older revision once confirmed", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /api/ }));
        expect(await screen.findByRole("heading", { name: "Service details: api" })).toBeInTheDocument();
        expect(api.runService).toHaveBeenCalledWith(target, "us-central1", "api");
        fireEvent.click(screen.getByRole("tab", { name: "Revisions" }));
        const revisions = await screen.findByRole("table", { name: "Revisions" });
        const old = within(revisions)
            .getAllByRole("row")
            .find((row) => row.textContent?.includes("api-00011-abc"))!;
        fireEvent.click(within(old).getByRole("button", { name: "Send all traffic" }));
        await waitFor(() => expect(api.runSetTraffic).toHaveBeenCalledWith(target, "us-central1", "api", "api-00011-abc"));
        expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ title: "Send all traffic to api-00011-abc?" }));
    });

    it("shows a service's environment variables by name, and where secret ones come from", async () => {
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /api/ }));
        fireEvent.click(await screen.findByRole("tab", { name: "Details" }));
        expect(await screen.findByText("DB_PASSWORD")).toBeInTheDocument();
        expect(screen.getByText("Secret Manager: db-password")).toBeInTheDocument();
        expect(screen.getByText("512Mi")).toBeInTheDocument();
    });

    it("executes a job from its page", async () => {
        api.runJobs.mockResolvedValue([
            {
                name: "nightly",
                location: "us-central1",
                state: "READY",
                image: "img",
                tasks: 1,
                executions: 3,
                latestExecution: "nightly-x",
                latestExecutionAt: null,
                updatedAt: null,
            },
        ]);
        api.runJob.mockResolvedValue({
            name: "nightly",
            location: "us-central1",
            state: "READY",
            image: "img",
            tasks: 1,
            executions: 3,
            latestExecution: "",
            latestExecutionAt: null,
            updatedAt: null,
            recent: [],
        });
        api.runJobExecute.mockResolvedValue({ operation: "op" });
        renderPage();
        fireEvent.click(await screen.findByRole("tab", { name: "Jobs" }));
        fireEvent.click(await screen.findByRole("row", { name: /nightly/ }));
        await act(async () => fireEvent.click(await screen.findByRole("button", { name: "Execute" })));
        await waitFor(() => expect(api.runJobExecute).toHaveBeenCalledWith(target, "us-central1", "nightly"));
    });

    it("deploys a new image, and new scaling, as new revisions", async () => {
        api.runDeploy.mockResolvedValue({ operation: "op" });
        prompt.mockResolvedValueOnce("us-docker.pkg.dev/acme/api:1.5");
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /api/ }));
        fireEvent.click(await screen.findByRole("button", { name: "Deploy image…" }));
        await waitFor(() => expect(api.runDeploy).toHaveBeenCalledWith(target, "us-central1", "api", { image: "us-docker.pkg.dev/acme/api:1.5" }));
        prompt.mockResolvedValueOnce("1").mockResolvedValueOnce("20");
        fireEvent.click(screen.getByRole("button", { name: "Edit scaling…" }));
        await waitFor(() => expect(api.runDeploy).toHaveBeenLastCalledWith(target, "us-central1", "api", { minInstances: 1, maxInstances: 20 }));
    });

    it("splits traffic between revisions once the shares add up to 100", async () => {
        api.runSplitTraffic.mockResolvedValue({ operation: "op" });
        renderPage();
        fireEvent.click(await screen.findByRole("row", { name: /api/ }));
        fireEvent.click(await screen.findByRole("tab", { name: "Revisions" }));
        fireEvent.click(await screen.findByRole("button", { name: "Manage traffic" }));
        fireEvent.change(screen.getByRole("spinbutton", { name: "Traffic to api-00012-xyz" }), { target: { value: "80" } });
        expect(screen.getByText("Total 90%")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
        fireEvent.change(screen.getByRole("spinbutton", { name: "Traffic to api-00011-abc" }), { target: { value: "20" } });
        fireEvent.click(screen.getByRole("button", { name: "Save" }));
        await waitFor(() =>
            expect(api.runSplitTraffic).toHaveBeenCalledWith(target, "us-central1", "api", [
                { revision: "api-00012-xyz", percent: 80 },
                { revision: "api-00011-abc", percent: 20 },
            ]),
        );
    });
});
