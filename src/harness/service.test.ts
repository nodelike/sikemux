import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { useStore } from "../state/store";
import { createProjectSession } from "../state/commands";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { loadProjectConfig } from "../projectConfig";
import { trustProjectConfig } from "../projectConfigRuntime";
import { handleHarnessRequest, harnessTasks, type HarnessRequest } from "./service";
import { withAgents } from "../test/agents";

vi.mock("../projectConfig", async (original) => ({ ...(await original<object>()), loadProjectConfig: vi.fn() }));
vi.mock("../projectConfigRuntime", async (original) => ({ ...(await original<object>()), trustProjectConfig: vi.fn() }));
const initial = useStore.getState();
let transport: MemoryIpcTransport;
const config = {
    status: "valid" as const,
    path: "/one/sikemux.json",
    fingerprint: "original",
    config: {
        version: 1 as const,
        actions: [],
        tasks: [{ id: "test", label: "Test", command: "echo test", cwd: ".", env: { PRIVATE: "not-in-inspection" } }],
        preview: { url: "http://localhost:5173", command: "echo test" },
    },
    trust: { requiresApproval: true, executableEntries: 1, reasons: [] },
};
function request(method: string, params = {}): HarnessRequest {
    return { id: crypto.randomUUID(), project: "/one", agentId: null, method, params };
}

beforeEach(() => {
    useStore.setState(initial, true);
    createProjectSession("/one");
    createProjectSession("/two");
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    vi.mocked(loadProjectConfig).mockResolvedValue(config);
    vi.mocked(trustProjectConfig).mockResolvedValue(true);
});
afterEach(() => {
    vi.restoreAllMocks();
    resetIpcTransportForTests();
    useStore.setState(initial, true);
});

describe("harness command service", () => {
    it("inspects only the requested project without task environment values", async () => {
        const result = await handleHarnessRequest(request("workspace.inspect"));
        expect(JSON.stringify(result)).not.toContain("not-in-inspection");
        expect(result).toMatchObject({ project: "/one", active: false, tasks: [{ id: "test" }] });
        await expect(handleHarnessRequest({ ...request("workspace.inspect"), project: "/missing" })).rejects.toThrow("not open");
        await expect(handleHarnessRequest({ ...request("workspace.inspect"), agentId: "other-agent" })).rejects.toThrow("does not belong");
    });
    it("queues a file open in an unmounted editor without stealing focus", async () => {
        transport.register("harness_resolve_path", () => "/one/file.ts");
        const before = useStore.getState().activeSessionId;
        await handleHarnessRequest(request("ui.open", { kind: "file", path: "file.ts", line: 12 }));
        const state = useStore.getState();
        expect(state.activeSessionId).toBe(before);
        expect(Object.values(state.editorViews).some((view) => view.openTabs.includes("/one/file.ts") && view.activePath === null)).toBe(true);
        expect(Object.values(state.pendingEditorOpens).flat()).toEqual([]);
        await handleHarnessRequest(request("ui.open", { kind: "file", path: "file.ts", line: 12, focus: true }));
        expect(Object.values(useStore.getState().pendingEditorOpens).flat()).toEqual([expect.objectContaining({ path: "/one/file.ts", line: 11 })]);
        await handleHarnessRequest(request("ui.open", { kind: "diff", focus: true }));
        expect(useStore.getState().sessions[useStore.getState().activeSessionId].cwd).toBe("/one");
    });
    it("rejects untrusted and changed configurations before launching", async () => {
        const start = vi.spyOn(harnessTasks, "start");
        vi.mocked(trustProjectConfig).mockResolvedValueOnce(false);
        await expect(handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "denied" }))).rejects.toThrow("not approved");
        vi.mocked(loadProjectConfig)
            .mockResolvedValueOnce(config)
            .mockResolvedValueOnce({ ...config, fingerprint: "changed" });
        await expect(handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "changed" }))).rejects.toThrow(
            "configuration changed",
        );
        expect(start).not.toHaveBeenCalled();
    });
    it("tells a missing configuration apart from an invalid one", async () => {
        const errors = [{ path: "$.tasks[0].cwd", code: "missing-field" as const, message: "is required" }];
        vi.mocked(loadProjectConfig).mockResolvedValue({ status: "absent", path: "/one/sikemux.json" });
        await expect(handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "absent" }))).rejects.toThrow(
            "has no sikemux.json",
        );
        expect(await handleHarnessRequest(request("workspace.inspect"))).toMatchObject({ configStatus: "absent", configErrors: undefined });
        vi.mocked(loadProjectConfig).mockResolvedValue({ status: "invalid", path: "/one/sikemux.json", errors });
        await expect(handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "invalid" }))).rejects.toThrow(
            "$.tasks[0].cwd is required",
        );
        expect(await handleHarnessRequest(request("workspace.inspect"))).toMatchObject({ configStatus: "invalid", configErrors: errors });
    });
    it("passes exact configured launch data and leaves focus in the other project", async () => {
        const start = vi.spyOn(harnessTasks, "start").mockResolvedValue({ executionId: "run", taskId: "test", project: "/one", status: "running" });
        await handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "new" }));
        expect(start).toHaveBeenCalledWith(
            expect.objectContaining({ command: "echo test", cwd: "/one", env: { PRIVATE: "not-in-inspection" } }),
            "new",
            "http://localhost:5173",
            undefined,
        );
        expect(useStore.getState().sessions[useStore.getState().activeSessionId].cwd).toBe("/two");
    });
    it("hands the requesting agent to the task, so its terminal goes on that agent's desk", async () => {
        const state = useStore.getState();
        const session = Object.values(state.sessions).find((session) => session.cwd === "/one")!;
        useStore.setState(withAgents(state, session.id, [{ id: "fixture-agent", type: "codex", title: "Fixture", startup: "" }]));
        const start = vi.spyOn(harnessTasks, "start").mockResolvedValue({ executionId: "run", taskId: "test", project: "/one", status: "running" });
        await handleHarnessRequest({ ...request("task.start", { taskId: "test", idempotencyKey: "desk" }), agentId: "fixture-agent" });
        expect(start).toHaveBeenCalledWith(expect.anything(), "desk", "http://localhost:5173", "fixture-agent");
    });
    it("opens a file an agent asks for on that agent's desk, at the line it names", async () => {
        const state = useStore.getState();
        const session = Object.values(state.sessions).find((session) => session.cwd === "/one")!;
        useStore.setState(withAgents(state, session.id, [{ id: "fixture-agent", type: "codex", title: "Fixture", startup: "" }]));
        transport.register("harness_resolve_path", () => "/one/file.ts");
        const result = await handleHarnessRequest({ ...request("ui.open", { kind: "file", path: "file.ts", line: 12 }), agentId: "fixture-agent" });
        expect(result).toEqual({ kind: "file", agentId: "fixture-agent", path: "/one/file.ts" });
        expect(useStore.getState().desks["fixture-agent"]).toMatchObject({
            active: "file:/one/file.ts",
            reveal: { path: "/one/file.ts", line: 11, character: 0 },
        });
        expect(useStore.getState().activeSessionId).toBe(state.activeSessionId);
    });
    it("opens the configured preview in the requesting agent's browser", async () => {
        const state = useStore.getState();
        const session = Object.values(state.sessions).find((session) => session.cwd === "/one")!;
        useStore.setState(withAgents(state, session.id, [{ id: "fixture-agent", type: "codex", title: "Fixture", startup: "" }]));
        const open = vi.fn(() => "fixture-tab");
        transport.register("browser_new_tab", open);
        const result = await handleHarnessRequest({ ...request("ui.open", { kind: "preview" }), agentId: "fixture-agent" });
        expect(result).toEqual({ kind: "preview", tabId: "fixture-tab", url: "http://localhost:5173" });
        expect(open).toHaveBeenCalledWith({ agentId: "fixture-agent", url: "http://localhost:5173" }, expect.anything());
        expect(useStore.getState().activeSessionId).toBe(state.activeSessionId);
    });
    it("reads a task's latest execution by task id and passes the read mode through", async () => {
        const runs = [
            { executionId: "old", taskId: "test", project: "/one", status: "stopped" as const, ptyId: 1 },
            { executionId: "new", taskId: "test", project: "/one", status: "running" as const, ptyId: 2 },
        ];
        vi.spyOn(harnessTasks, "list").mockReturnValue(runs);
        vi.spyOn(harnessTasks, "get").mockImplementation((_, id) => runs.find((run) => run.executionId === id)!);
        const output = vi.fn(() => ({
            bytes: [...new TextEncoder().encode("ready\n")],
            cursor: 6,
            end: 6,
            hasMore: false,
            truncated: false,
            matches: 1,
        }));
        transport.register("harness_task_output", output);
        const result = await handleHarnessRequest(request("task.read", { taskId: "test", search: "ready", tail: 1, plain: true }));
        expect(result).toMatchObject({ executionId: "new", output: "ready\n", end: 6, matches: 1 });
        expect(output).toHaveBeenCalledWith(
            { id: 2, query: { cursor: 0, limit: 8192, tail: 1, search: "ready", context: 3, plain: true } },
            expect.anything(),
        );
        await expect(handleHarnessRequest(request("task.read", { taskId: "test", executionId: "new" }))).rejects.toThrow("not both");
        await expect(handleHarnessRequest(request("task.read", {}))).rejects.toThrow("executionId or taskId");
        await expect(handleHarnessRequest(request("task.read", { taskId: "other" }))).rejects.toThrow("has not been started");
        const stop = vi.spyOn(harnessTasks, "stop").mockResolvedValue(runs[1]);
        await handleHarnessRequest(request("task.stop", { taskId: "test" }));
        expect(stop).toHaveBeenCalledWith("/one", "new");
    });
    it("restarts a task by stopping its latest execution before starting a fresh one", async () => {
        const running = { executionId: "old", taskId: "test", project: "/one", status: "running" as const, ptyId: 1 };
        vi.spyOn(harnessTasks, "list").mockReturnValue([running]);
        const order: string[] = [];
        const stop = vi.spyOn(harnessTasks, "stop").mockImplementation(async () => {
            order.push("stop");
            return { ...running, status: "stopped" };
        });
        const start = vi.spyOn(harnessTasks, "start").mockImplementation(async () => {
            order.push("start");
            return { executionId: "fresh", taskId: "test", project: "/one", status: "running" };
        });
        expect(await handleHarnessRequest(request("task.restart", { taskId: "test" }))).toMatchObject({ executionId: "fresh" });
        expect(stop).toHaveBeenCalledWith("/one", "old");
        expect(order).toEqual(["stop", "start"]);
        expect(start.mock.calls[0][1]).toMatch(/^[0-9a-f-]{36}$/);
    });
    it("waits for readyWhen text to appear in a started task's output", async () => {
        const run = { executionId: "run", taskId: "test", project: "/one", status: "running" as const, ptyId: 7 };
        vi.spyOn(harnessTasks, "start").mockResolvedValue(run);
        vi.spyOn(harnessTasks, "get").mockReturnValue(run);
        const output = vi.fn(() => ({ bytes: [], cursor: 0, end: 20, hasMore: false, truncated: false, matches: 1 }));
        transport.register("harness_task_output", output);
        const result = await handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "ready", readyWhen: "Ready in" }));
        expect(result).toMatchObject({ executionId: "run", ready: true });
        expect(output).toHaveBeenCalledWith({ id: 7, query: expect.objectContaining({ search: "Ready in", tail: 1 }) }, expect.anything());
        vi.spyOn(harnessTasks, "get").mockReturnValue({ ...run, status: "failed" });
        output.mockReturnValue({ bytes: [], cursor: 0, end: 20, hasMore: false, truncated: false, matches: 0 });
        const failed = await handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "never", readyWhen: "Ready in" }));
        expect(failed).toMatchObject({ ready: false });
    });
    it("rejects invalid read limits and preview opens without an agent", async () => {
        vi.spyOn(harnessTasks, "get").mockReturnValue({ executionId: "run", taskId: "test", project: "/one", status: "running", ptyId: 42 });
        await expect(handleHarnessRequest(request("task.read", { executionId: "run", limit: 99999 }))).rejects.toThrow("limit");
        await expect(handleHarnessRequest(request("ui.open", { kind: "preview" }))).rejects.toThrow("agent session");
    });
});
