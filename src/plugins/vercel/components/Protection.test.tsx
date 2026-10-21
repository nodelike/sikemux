import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelDeployment } from "../api";

const api = vi.hoisted(() => ({ protection: vi.fn(), shareLink: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn(), copyText: vi.fn(() => Promise.resolve()) }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { Protection } from "./Protection";
import { vercel } from "../resources";

const preview = { id: "dpl_1", project: "web", projectId: "prj_1", url: "https://web-abc.vercel.app" } as VercelDeployment;
const LINK = "https://web-abc.vercel.app/?x-vercel-protection-bypass=s3cr3t&x-vercel-set-bypass-cookie=samesitenone";

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockClear());
    host.confirmDialog.mockResolvedValue(true);
    api.shareLink.mockResolvedValue({ link: LINK });
});

describe("deployment protection", () => {
    it("says what a deployment asks for, and copies a link past it without showing it", async () => {
        api.protection.mockResolvedValue({ vercelAuthentication: "Preview deployments", password: null, canShare: true });
        render(<Protection active team="team_acme" deployment={preview} />);
        expect(await screen.findByText("Vercel Authentication on preview deployments")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Copy share link" }));
        await waitFor(() => expect(host.copyText).toHaveBeenCalledWith(LINK));
        expect(host.confirmDialog).not.toHaveBeenCalled();
        expect(screen.queryByText(/s3cr3t/)).not.toBeInTheDocument();
    });

    it("asks before making the project's first sharing secret", async () => {
        api.protection.mockResolvedValue({ vercelAuthentication: "All deployments", password: null, canShare: false });
        host.confirmDialog.mockResolvedValue(false);
        render(<Protection active team="team_acme" deployment={preview} />);
        fireEvent.click(await screen.findByRole("button", { name: "Copy share link" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ title: "Make a share link?" })));
        expect(api.shareLink).not.toHaveBeenCalled();
    });

    it("offers no link when nothing guards the deployment", async () => {
        api.protection.mockResolvedValue({ vercelAuthentication: null, password: null, canShare: false });
        render(<Protection active team="team_acme" deployment={preview} />);
        expect(await screen.findByText(/Anyone with the address can open it/)).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Copy share link" })).not.toBeInTheDocument();
    });
});
