import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConfirmRequest } from "../state/dialog";
import { clearProjectConfigTrustForTests, trustProjectConfig } from "./projectConfigRuntime";

beforeEach(clearProjectConfigTrustForTests);

describe("project config runtime boundary", () => {
    it("asks once per exact fingerprint", async () => {
        const confirm = vi.fn(async () => true);
        const result = {
            status: "valid" as const,
            path: "/repo/sikemux.json",
            fingerprint: "sha256:one",
            config: { version: 1 as const, actions: [], tasks: [] },
            trust: { requiresApproval: true, executableEntries: 1, reasons: ["a project action"] },
        };
        await expect(trustProjectConfig(result, confirm)).resolves.toBe(true);
        await expect(trustProjectConfig(result, confirm)).resolves.toBe(true);
        expect(confirm).toHaveBeenCalledTimes(1);
    });

    it("does not remember rejected trust", async () => {
        const confirm = vi.fn(async () => false);
        const result = {
            status: "valid" as const,
            path: "/repo/sikemux.json",
            fingerprint: "sha256:two",
            config: { version: 1 as const, actions: [], tasks: [] },
            trust: { requiresApproval: true, executableEntries: 1, reasons: ["a hook"] },
        };
        await expect(trustProjectConfig(result, confirm)).resolves.toBe(false);
        await expect(trustProjectConfig(result, confirm)).resolves.toBe(false);
        expect(confirm).toHaveBeenCalledTimes(2);
    });

    it("scopes trust to both content and project path", async () => {
        const confirm = vi.fn(async () => true);
        const base = {
            status: "valid" as const,
            fingerprint: "sha256:same",
            config: { version: 1 as const, actions: [], tasks: [] },
            trust: { requiresApproval: true, executableEntries: 1, reasons: ["an action"] },
        };
        await expect(trustProjectConfig({ ...base, path: "/repo-a/sikemux.json" }, confirm)).resolves.toBe(true);
        await expect(trustProjectConfig({ ...base, path: "/repo-b/sikemux.json" }, confirm)).resolves.toBe(true);
        expect(confirm).toHaveBeenCalledTimes(2);
    });

    it("lists every command the config would be allowed to run", async () => {
        const confirm = vi.fn(async (_request: ConfirmRequest) => true);
        await trustProjectConfig(
            {
                status: "valid",
                path: "/repo/sikemux.json",
                fingerprint: "sha256:three",
                config: {
                    version: 1,
                    actions: [{ id: "test", label: "Test", description: "", command: "pnpm test", placement: "popup", contexts: [] }],
                    tasks: [{ id: "web", label: "Web", command: "pnpm dev", cwd: "apps/web", env: { PORT: "3000" } }],
                    preview: { url: "http://localhost:3000", command: "pnpm preview" },
                    worktree: { onCreate: [{ id: "deps", label: "Install", command: "pnpm install" }] },
                },
                trust: { requiresApproval: true, executableEntries: 4, reasons: [] },
            },
            confirm,
        );
        expect(confirm.mock.calls[0][0].commands).toEqual([
            { label: "Action · Test", command: "pnpm test" },
            { label: "Task · Web", command: "cd apps/web && PORT=3000 pnpm dev" },
            { label: "Preview", command: "pnpm preview" },
            { label: "New worktree · Install", command: "pnpm install" },
        ]);
    });
});
