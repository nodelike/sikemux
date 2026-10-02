import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { useStore } from "../state/store";
import { createProjectSession } from "../state/commands";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { loadProjectConfig } from "../projects/projectConfig";
import { trustProjectConfig } from "../projects/projectConfigRuntime";
import { appTaskRuntime } from "../tasks/application";
import { handleHarnessRequest, type HarnessRequest } from "./service";
import { withAgents } from "../test/agents";

vi.mock("@tauri-apps/api/core", async (original) => ({
    ...(await original<object>()),
    Channel: class {
        onmessage = () => {};
    },
}));
vi.mock("../projects/projectConfig", async (original) => ({ ...(await original<object>()), loadProjectConfig: vi.fn() }));
vi.mock("../projects/projectConfigRuntime", async (original) => ({ ...(await original<object>()), trustProjectConfig: vi.fn() }));
const initial = useStore.getState();
let transport: MemoryIpcTransport;
let spawn: ReturnType<typeof vi.fn<() => { ptyId: number }>>;
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
function start(params: Record<string, unknown> = {}): HarnessRequest {
    return request("task.start", { executionId: "run-1", taskId: "test", ...params });
}
function addAgents(agents: Parameters<typeof withAgents>[2]) {
    const state = useStore.getState();
    const session = Object.values(state.sessions).find((session) => session.cwd === "/one")!;
    useStore.setState(withAgents(state, session.id, agents));
    return state;
}

beforeEach(() => {
    useStore.setState(initial, true);
    createProjectSession("/one");
    createProjectSession("/two");
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    spawn = vi.fn<() => { ptyId: number }>(() => ({ ptyId: 5 }));
    transport.register("task_spawn", spawn);
    vi.mocked(loadProjectConfig).mockResolvedValue(config);
    vi.mocked(trustProjectConfig).mockResolvedValue(true);
});
afterEach(() => {
    vi.restoreAllMocks();
    resetIpcTransportForTests();
    useStore.setState(initial, true);
});

describe("harness window service", () => {
    it("inspects only the requested project, without task environment values or the core's runs", async () => {
        const result = await handleHarnessRequest(request("workspace.inspect"));
        expect(JSON.stringify(result)).not.toContain("not-in-inspection");
        expect(result).toMatchObject({ project: "/one", active: false, tasks: [{ id: "test" }] });
        expect(result).not.toHaveProperty("runs");
        expect(result).not.toHaveProperty("cursor");
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
        vi.mocked(trustProjectConfig).mockResolvedValueOnce(false);
        await expect(handleHarnessRequest(start())).rejects.toThrow("not approved");
        vi.mocked(loadProjectConfig)
            .mockResolvedValueOnce(config)
            .mockResolvedValueOnce({ ...config, fingerprint: "changed" });
        await expect(handleHarnessRequest(start())).rejects.toThrow("configuration changed");
        expect(spawn).not.toHaveBeenCalled();
    });
    it("tells the core when it asks the person to trust sikemux.json", async () => {
        const awaiting = vi.fn();
        transport.register("harness_awaiting_trust", awaiting);
        vi.mocked(trustProjectConfig).mockImplementationOnce(async (_config, ask) => {
            void ask!({ title: "Trust" });
            return true;
        });
        await handleHarnessRequest(start());
        expect(awaiting).toHaveBeenCalledWith({ executionId: "run-1" }, expect.anything());
    });
    it("starts a YOLO agent's task without asking the person to trust sikemux.json, and still asks for a safe agent", async () => {
        addAgents([
            { id: "yolo-agent", type: "claude", title: "Yolo", startup: "", permissionMode: "bypass" },
            { id: "safe-agent", type: "claude", title: "Safe", startup: "", permissionMode: "workspace-write" },
        ]);
        vi.mocked(trustProjectConfig).mockResolvedValue(false);
        await handleHarnessRequest({ ...start(), agentId: "yolo-agent" });
        expect(trustProjectConfig).not.toHaveBeenCalled();
        expect(spawn).toHaveBeenCalledTimes(1);
        await expect(handleHarnessRequest({ ...start({ executionId: "run-2" }), agentId: "safe-agent" })).rejects.toThrow("not approved");
        expect(trustProjectConfig).toHaveBeenCalledTimes(1);
    });
    it("tells a missing configuration apart from an invalid one", async () => {
        const errors = [{ path: "$.tasks[0].cwd", code: "missing-field" as const, message: "is required" }];
        vi.mocked(loadProjectConfig).mockResolvedValue({ status: "absent", path: "/one/sikemux.json" });
        await expect(handleHarnessRequest(start())).rejects.toThrow("has no sikemux.json");
        expect(await handleHarnessRequest(request("workspace.inspect"))).toMatchObject({ configStatus: "absent", configErrors: undefined });
        vi.mocked(loadProjectConfig).mockResolvedValue({ status: "invalid", path: "/one/sikemux.json", errors });
        await expect(handleHarnessRequest(start())).rejects.toThrow("$.tasks[0].cwd is required");
        expect(await handleHarnessRequest(request("workspace.inspect"))).toMatchObject({ configStatus: "invalid", configErrors: errors });
    });
    it("spawns the configured task under the core's execution id and leaves focus in the other project", async () => {
        expect(await handleHarnessRequest(start())).toEqual({ previewUrl: "http://localhost:5173" });
        expect(spawn).toHaveBeenCalledWith(
            {
                request: expect.objectContaining({
                    executionId: "run-1",
                    terminalKey: JSON.stringify(["harness", "/one", "test"]),
                    command: "echo test",
                    cwd: "/one",
                    env: { PRIVATE: "not-in-inspection" },
                }),
                onExit: expect.anything(),
            },
            expect.anything(),
        );
        expect(useStore.getState().sessions[useStore.getState().activeSessionId].cwd).toBe("/two");
    });
    it("runs a command the core resolved without asking for trust", async () => {
        const result = await handleHarnessRequest(start({ taskId: "sh:web-123abc", command: "pnpm dev --port 3000", cwd: "/one/web", label: "Web" }));
        expect(result).toEqual({});
        expect(spawn).toHaveBeenCalledWith(
            {
                request: expect.objectContaining({
                    taskId: "sh:web-123abc",
                    label: "Web",
                    command: "pnpm dev --port 3000",
                    cwd: "/one/web",
                    env: {},
                }),
                onExit: expect.anything(),
            },
            expect.anything(),
        );
        expect(trustProjectConfig).not.toHaveBeenCalled();
    });
    it("stops the run it replaces before it spawns the new one", async () => {
        const order: string[] = [];
        transport.register("harness_stop_runs", (args: unknown) => {
            order.push(`stop ${JSON.stringify(args)}`);
        });
        spawn.mockImplementation(() => {
            order.push("spawn");
            return { ptyId: 6 };
        });
        await handleHarnessRequest(start({ previousExecutionId: "old" }));
        expect(order).toEqual(['stop {"executionId":"old"}', "spawn"]);
    });
    it("leaves a task the command deck is running alone", async () => {
        vi.spyOn(appTaskRuntime, "getSnapshot").mockReturnValue({ status: "running", task: { id: "test" } } as ReturnType<
            typeof appTaskRuntime.getSnapshot
        >);
        await expect(handleHarnessRequest(start())).rejects.toThrow("command deck");
        expect(spawn).not.toHaveBeenCalled();
    });
    it("opens a file an agent asks for on that agent's desk, at the line it names", async () => {
        const state = addAgents([{ id: "fixture-agent", type: "codex", title: "Fixture", startup: "" }]);
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
        const state = addAgents([{ id: "fixture-agent", type: "codex", title: "Fixture", startup: "" }]);
        const open = vi.fn(() => "fixture-tab");
        transport.register("browser_new_tab", open);
        const result = await handleHarnessRequest({ ...request("ui.open", { kind: "preview" }), agentId: "fixture-agent" });
        expect(result).toEqual({ kind: "preview", tabId: "fixture-tab", url: "http://localhost:5173" });
        expect(open).toHaveBeenCalledWith({ agentId: "fixture-agent", url: "http://localhost:5173" }, expect.anything());
        expect(useStore.getState().activeSessionId).toBe(state.activeSessionId);
    });
    it("rejects preview opens without an agent and methods the core answers itself", async () => {
        await expect(handleHarnessRequest(request("ui.open", { kind: "preview" }))).rejects.toThrow("agent session");
        await expect(handleHarnessRequest(request("task.read", { taskId: "test" }))).rejects.toThrow("Unknown harness method");
    });
});
