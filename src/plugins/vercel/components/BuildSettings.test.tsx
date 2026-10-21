import { vercel } from "../resources";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelSettings } from "../api";

const api = vi.hoisted(() => ({ settings: vi.fn(), updateSettings: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { BuildSettings, changesOf } from "./BuildSettings";

const web: VercelSettings = {
    id: "prj_1",
    name: "web",
    framework: "nextjs",
    buildCommand: "pnpm build",
    outputDirectory: null,
    installCommand: null,
    devCommand: null,
    rootDirectory: "apps/web",
    nodeVersion: "22.x",
    nodeVersions: ["24.x", "22.x", "20.x"],
};

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => {
    api.settings.mockReset().mockResolvedValue(web);
    api.updateSettings.mockReset().mockResolvedValue(web);
    Object.values(host).forEach((mock) => mock.mockReset());
    host.confirmDialog.mockResolvedValue(true);
});

describe("a project's build settings", () => {
    it("sends only what changed, with an emptied command going back to the default", () => {
        const before = {
            framework: "nextjs",
            nodeVersion: "22.x",
            buildCommand: "pnpm build",
            outputDirectory: "",
            installCommand: "",
            devCommand: "",
            rootDirectory: "apps/web",
        };
        expect(changesOf(before, { ...before, buildCommand: "", outputDirectory: " out " })).toEqual({ buildCommand: null, outputDirectory: "out" });
        expect(changesOf(before, before)).toEqual({});
    });

    it("saves a changed command after asking, and discards back to what is saved", async () => {
        render(<BuildSettings active team="team_acme" project="web" />);
        const build = await screen.findByRole("textbox", { name: "Build Command" });
        expect(build).toHaveValue("pnpm build");
        const save = screen.getByRole("button", { name: "Save" });
        expect(save).toBeDisabled();
        fireEvent.change(build, { target: { value: "turbo run build" } });
        fireEvent.click(screen.getByRole("button", { name: "Discard" }));
        expect(build).toHaveValue("pnpm build");
        fireEvent.change(build, { target: { value: "turbo run build" } });
        fireEvent.click(save);
        await waitFor(() => expect(api.updateSettings).toHaveBeenCalledWith("team_acme", "web", { buildCommand: "turbo run build" }));
        expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ title: "Save web's settings?" }));
        await waitFor(() => expect(host.notify).toHaveBeenCalledWith("success", "Saved web's settings"));
    });

    it("saves nothing when the person says no", async () => {
        host.confirmDialog.mockResolvedValue(false);
        render(<BuildSettings active team="team_acme" project="web" />);
        fireEvent.change(await screen.findByRole("textbox", { name: "Root Directory" }), { target: { value: "" } });
        fireEvent.click(screen.getByRole("button", { name: "Save" }));
        await waitFor(() => expect(host.confirmDialog).toHaveBeenCalled());
        expect(api.updateSettings).not.toHaveBeenCalled();
    });
});
