import { describe, expect, it, vi } from "vitest";
import {
    fingerprintProjectConfigSource,
    loadProjectConfig,
    projectConfigTrustSummary,
    validateProjectConfig,
    type SikemuxProjectConfig,
} from "./projectConfig";

const minimal = { version: 1 };

describe("project configuration", () => {
    it("distinguishes an absent configuration and reports its normalized path", async () => {
        const read = vi.fn(async () => {
            throw new Error("No such file or directory (os error 2)");
        });

        await expect(loadProjectConfig("C:\\work\\app\\", read)).resolves.toEqual({
            status: "absent",
            path: "C:/work/app/sikemux.json",
        });
        expect(read).toHaveBeenCalledWith("C:/work/app/sikemux.json");
    });

    it("does not misclassify permission and I/O failures as absence", async () => {
        const result = await loadProjectConfig("/repo", async () => {
            throw new Error("Permission denied");
        });
        expect(result).toMatchObject({ status: "invalid", path: "/repo/sikemux.json" });
        if (result.status === "invalid") expect(result.errors[0]).toMatchObject({ code: "read-failed", path: "$" });
    });

    it("loads and normalizes every supported section", async () => {
        const source = JSON.stringify({
            version: 1,
            $schema: "https://example.test/sikemux.schema.json",
            icon: "assets\\project.svg",
            actions: [
                {
                    id: "quality.test",
                    label: "Test",
                    description: "Run the focused suite",
                    command: "pnpm test",
                    placement: "split",
                    contexts: ["project", "project"],
                    keybinding: "Meta+Shift+KeyT",
                },
            ],
            tasks: [
                {
                    id: "quality.watch",
                    label: "Watch tests",
                    command: "pnpm test --watch",
                    cwd: "packages\\app/",
                    env: { NODE_ENV: "test", FORCE_COLOR: "1" },
                },
            ],
            preview: { url: "http://localhost:4173", command: "pnpm preview" },
            worktree: { onCreate: [{ id: "deps", label: "Install dependencies", command: "pnpm install" }] },
        });
        const result = await loadProjectConfig("/repo/", async () => source);

        expect(result).toMatchObject({
            status: "valid",
            path: "/repo/sikemux.json",
            config: {
                icon: "assets/project.svg",
                actions: [{ id: "quality.test", placement: "split", contexts: ["project"] }],
                tasks: [
                    {
                        id: "quality.watch",
                        label: "Watch tests",
                        command: "pnpm test --watch",
                        cwd: "packages/app",
                        env: { NODE_ENV: "test", FORCE_COLOR: "1" },
                    },
                ],
                preview: { url: "http://localhost:4173", command: "pnpm preview" },
                worktree: { onCreate: [{ id: "deps", label: "Install dependencies", command: "pnpm install" }] },
            },
            trust: { requiresApproval: true, executableEntries: 4 },
        });
        if (result.status === "valid") expect(result.fingerprint).toMatch(/^sha256:[a-f0-9]{64}$/);
    });

    it("applies inert defaults to optional actions fields", () => {
        const result = validateProjectConfig({ version: 1, actions: [{ id: "check", label: "Check", command: "pnpm check" }] });
        expect(result).toEqual({
            ok: true,
            config: {
                version: 1,
                actions: [{ id: "check", label: "Check", description: "", command: "pnpm check", placement: "terminal", contexts: [] }],
                tasks: [],
            },
        });
    });

    it("rejects session contexts that project actions cannot execute in", () => {
        for (const context of ["command", "ssh", "aws", "rundeck", "bruno"]) {
            const result = validateProjectConfig({
                version: 1,
                actions: [{ id: "check", label: "Check", command: "pnpm check", contexts: [context] }],
            });
            expect(result).toEqual({
                ok: false,
                errors: [
                    {
                        path: "$.actions[0].contexts[0]",
                        code: "invalid-value",
                        message: 'Project action context must be "project".',
                    },
                ],
            });
        }
    });

    it("normalizes safe action keybindings and rejects typing-like or malformed bindings", () => {
        const normalized = validateProjectConfig({
            version: 1,
            actions: [{ id: "check", label: "Check", command: "pnpm check", keybinding: "Shift+Meta+KeyT" }],
        });
        expect(normalized).toMatchObject({ ok: true, config: { actions: [{ keybinding: "Meta+Shift+KeyT" }] } });

        for (const keybinding of ["KeyA", "Shift+KeyA", "Meta+Garbage", "Meta+KeyA+KeyB", "Meta+Meta+KeyA"]) {
            const result = validateProjectConfig({
                version: 1,
                actions: [{ id: "check", label: "Check", command: "pnpm check", keybinding }],
            });
            expect(result).toMatchObject({
                ok: false,
                errors: [expect.objectContaining({ path: "$.actions[0].keybinding", code: "invalid-value" })],
            });
        }
    });

    it("returns actionable JSON parse errors with a fingerprint", async () => {
        const result = await loadProjectConfig("/repo", async () => '{"version":');
        expect(result).toMatchObject({ status: "invalid", path: "/repo/sikemux.json", errors: [{ code: "invalid-json", path: "$" }] });
        if (result.status === "invalid") expect(result.fingerprint).toMatch(/^sha256:/);
    });

    it("rejects unsupported and missing versions", () => {
        expect(validateProjectConfig({}).ok).toBe(false);
        const result = validateProjectConfig({ version: 2 });
        expect(result).toEqual({
            ok: false,
            errors: [
                {
                    path: "$.version",
                    code: "unsupported-version",
                    message: "Unsupported project configuration version “2”; expected 1.",
                },
            ],
        });
    });

    it("rejects unknown fields at every supported nesting level", () => {
        const result = validateProjectConfig({
            version: 1,
            surprise: true,
            actions: [{ id: "test", label: "Test", command: "test", shell: "bash" }],
            preview: { url: "http://localhost:3000", open: true },
            worktree: { onCreate: [{ id: "setup", command: "setup", cwd: "/tmp" }], automatic: true },
        });
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.errors.map(({ code, path }) => ({ code, path }))).toEqual(
                expect.arrayContaining([
                    { code: "unknown-field", path: "$.surprise" },
                    { code: "unknown-field", path: "$.actions[0].shell" },
                    { code: "unknown-field", path: "$.preview.open" },
                    { code: "unknown-field", path: "$.worktree.automatic" },
                    { code: "unknown-field", path: "$.worktree.onCreate[0].cwd" },
                ]),
            );
        }
    });

    it("rejects unsafe icon paths and credential-bearing preview URLs", () => {
        const result = validateProjectConfig({
            version: 1,
            icon: "../outside.svg",
            preview: { url: "https://user:password@example.test" },
        });
        expect(result.ok).toBe(false);
        if (!result.ok) expect(result.errors.map((error) => error.path)).toEqual(expect.arrayContaining(["$.icon", "$.preview.url"]));
    });

    it("rejects duplicate IDs, malformed contexts, placements, and hooks", () => {
        const result = validateProjectConfig({
            version: 1,
            actions: [
                { id: "same", label: "One", command: "one" },
                { id: "same", label: "Two", command: "two", contexts: ["not-a-context"], placement: "window" },
            ],
            tasks: [
                { id: "same", label: "One", command: "one" },
                { id: "same", label: "Two", command: "two", cwd: "../outside", env: { "BAD-KEY": 42 } },
            ],
            worktree: {
                onCreate: [
                    { id: "setup", command: "one" },
                    { id: "setup", command: "two" },
                ],
            },
        });
        expect(result.ok).toBe(false);
        if (!result.ok) {
            expect(result.errors.map((error) => error.code)).toEqual(expect.arrayContaining(["duplicate-id", "invalid-value"]));
            expect(result.errors.filter((error) => error.code === "duplicate-id")).toHaveLength(3);
            expect(result.errors.map((error) => error.path)).toEqual(expect.arrayContaining(["$.tasks[1].cwd", "$.tasks[1].env.BAD-KEY"]));
        }
    });

    it("requires preview content and rejects non-HTTP protocols", () => {
        const empty = validateProjectConfig({ version: 1, preview: {} });
        const file = validateProjectConfig({ version: 1, preview: { url: "file:///etc/passwd" } });
        expect(empty.ok).toBe(false);
        expect(file.ok).toBe(false);
    });

    it("fingerprints exact source content and summarizes trust without executing it", async () => {
        await expect(fingerprintProjectConfigSource("same")).resolves.toBe(await fingerprintProjectConfigSource("same"));
        await expect(fingerprintProjectConfigSource("changed")).resolves.not.toBe(await fingerprintProjectConfigSource("same"));
        const config: SikemuxProjectConfig = {
            version: 1,
            actions: [],
            tasks: [],
            preview: { url: "http://localhost:3000" },
            worktree: { onCreate: [] },
        };
        expect(projectConfigTrustSummary(config)).toEqual({ requiresApproval: false, executableEntries: 0, reasons: [] });
    });

    it("rejects oversized files before attempting to parse them", async () => {
        const result = await loadProjectConfig("/repo", async () => "x".repeat(256 * 1024 + 1));
        expect(result).toMatchObject({ status: "invalid", errors: [{ code: "limit-exceeded", path: "$" }] });
    });

    it("accepts the smallest valid configuration", async () => {
        const result = await loadProjectConfig("/repo", async () => JSON.stringify(minimal));
        expect(result).toMatchObject({ status: "valid", config: { version: 1, actions: [], tasks: [] }, trust: { requiresApproval: false } });
    });
});
