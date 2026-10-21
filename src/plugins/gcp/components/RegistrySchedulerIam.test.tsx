import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const host = vi.hoisted(() => ({ confirm: vi.fn(() => Promise.resolve(true)), copy: vi.fn() }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    confirmDialog: host.confirm,
    copyWithNotice: host.copy,
}));

import { answerEmpty } from "../testing";
import { IamPage } from "./IamPage";
import { RegistryPage } from "./RegistryPage";
import { SchedulerPage } from "./SchedulerPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.repositories.mockResolvedValue([
        {
            name: "web",
            location: "us",
            format: "DOCKER",
            mode: "STANDARD_REPOSITORY",
            description: "Website images",
            sizeBytes: 5_000_000_000,
            updatedAt: null,
        },
    ]);
    api.repository.mockResolvedValue({
        format: "DOCKER",
        images: [
            {
                digest: "sha256:abc123def4567890",
                image: "storefront",
                uri: "us-docker.pkg.dev/acme-prod/web/storefront@sha256:abc123def4567890",
                tags: ["2.4.0", "latest"],
                sizeBytes: 120_000_000,
                uploadedAt: null,
                builtAt: null,
            },
        ],
    });
    api.schedulerJobs.mockResolvedValue([
        {
            name: "nightly-export",
            location: "us-central1",
            description: null,
            schedule: "0 3 * * *",
            timeZone: "Europe/London",
            state: "ENABLED",
            target: { kind: "http", method: "POST", uri: "https://export.run.app" },
            lastRunAt: "2024-10-18T03:00:00Z",
            nextRunAt: null,
            lastRunOk: false,
            lastRunError: "timed out",
        },
    ]);
    api.schedulerChange.mockResolvedValue({});
    api.iamMembers.mockResolvedValue([{ member: "edon@acme.dev", kind: "user", roles: ["owner", "run.admin"] }]);
    host.confirm.mockClear();
    host.copy.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Artifact Registry", () => {
    it("opens a repository and copies an image's pinned address", async () => {
        render(<RegistryPage paneId="ar-1" active target={target} />);
        fireEvent.click(await screen.findByRole("row", { name: /web/ }));
        const images = await screen.findByRole("table", { name: "Images" });
        expect(within(images).getByText("latest")).toBeInTheDocument();
        fireEvent.click(within(images).getByRole("row", { name: /storefront/ }));
        expect(host.copy).toHaveBeenCalledWith("us-docker.pkg.dev/acme-prod/web/storefront@sha256:abc123def4567890", "the image address");
    });
});

describe("Cloud Scheduler", () => {
    it("shows a failed last run and forces a run once confirmed", async () => {
        render(<SchedulerPage paneId="cs-1" active target={target} />);
        expect(await screen.findByText(/failed/)).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Force run" }));
        await waitFor(() => expect(api.schedulerChange).toHaveBeenCalledWith(target, "us-central1", "nightly-export", "run"));
        fireEvent.click(screen.getByRole("button", { name: "Pause" }));
        await waitFor(() => expect(api.schedulerChange).toHaveBeenLastCalledWith(target, "us-central1", "nightly-export", "pause"));
    });
});

describe("IAM", () => {
    it("lists each member with the roles they hold, filtered by role", async () => {
        render(<IamPage paneId="iam-1" active target={target} />);
        expect(await screen.findByText("run.admin")).toBeInTheDocument();
        fireEvent.change(screen.getByRole("textbox", { name: "Filter by member or role" }), { target: { value: "viewer" } });
        expect(screen.queryByText("edon@acme.dev")).not.toBeInTheDocument();
    });
});
