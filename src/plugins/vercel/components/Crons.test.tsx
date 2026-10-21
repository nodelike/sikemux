import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelProject } from "../api";

const api = vi.hoisted(() => ({ crons: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));

import { Crons } from "./Crons";
import { vercel } from "../resources";

const project = { name: "web", production: { url: "https://shop.acme.dev" } } as VercelProject;

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => api.crons.mockReset());

describe("cron jobs", () => {
    it("list each job's path and when it runs, in words", async () => {
        api.crons.mockResolvedValue({
            enabled: true,
            deploymentId: "dpl_9",
            updatedAt: null,
            jobs: [{ path: "/api/cron/sync", schedule: "0 9 * * *", host: null }],
        });
        render(<Crons active team="team_acme" project={project} />);
        const row = await screen.findByRole("row", { name: "/api/cron/sync" });
        expect(within(row).getByText("Every day at 09:00 UTC")).toBeInTheDocument();
        expect(within(row).getByRole("button", { name: "Open" })).toBeInTheDocument();
    });

    it("say how to add one when there are none", async () => {
        api.crons.mockResolvedValue({ enabled: false, deploymentId: null, updatedAt: null, jobs: [] });
        render(<Crons active team="team_acme" project={project} />);
        expect(await screen.findByText(/No cron jobs/)).toBeInTheDocument();
    });
});
