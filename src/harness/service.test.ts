import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { useStore } from "../state/store";
import { createProjectSession } from "../state/commands";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { loadProjectConfig } from "../projectConfig";
import { trustProjectConfig } from "../projectConfigRuntime";
import { handleHarnessRequest, harnessTasks, type HarnessRequest } from "./service";

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
    it("passes exact configured launch data and leaves focus in the other project", async () => {
        const start = vi.spyOn(harnessTasks, "start").mockResolvedValue({ executionId: "run", taskId: "test", project: "/one", status: "running" });
        await handleHarnessRequest(request("task.start", { taskId: "test", idempotencyKey: "new" }));
        expect(start).toHaveBeenCalledWith(
            expect.objectContaining({ command: "echo test", cwd: "/one", env: { PRIVATE: "not-in-inspection" } }),
            "new",
            "http://localhost:5173",
        );
        expect(useStore.getState().sessions[useStore.getState().activeSessionId].cwd).toBe("/two");
    });
    it("opens the configured preview in the requesting agent's browser", async () => {
        const state = useStore.getState();
        const session = Object.values(state.sessions).find((session) => session.cwd === "/one")!;
        useStore.setState({
            agents: { ...state.agents, "fixture-agent": { id: "fixture-agent", type: "codex", title: "Fixture", startup: "" } },
            agentsBySession: { ...state.agentsBySession, [session.id]: ["fixture-agent"] },
        });
        const open = vi.fn(() => "fixture-tab");
        transport.register("browser_new_tab", open);
        const result = await handleHarnessRequest({ ...request("ui.open", { kind: "preview" }), agentId: "fixture-agent" });
        expect(result).toEqual({ kind: "preview", tabId: "fixture-tab", url: "http://localhost:5173" });
        expect(open).toHaveBeenCalledWith({ agentId: "fixture-agent", url: "http://localhost:5173" }, expect.anything());
        expect(useStore.getState().activeSessionId).toBe(state.activeSessionId);
    });
    it("rejects invalid read limits and preview opens without an agent", async () => {
        vi.spyOn(harnessTasks, "get").mockReturnValue({ executionId: "run", taskId: "test", project: "/one", status: "running", ptyId: 42 });
        await expect(handleHarnessRequest(request("task.read", { executionId: "run", limit: 99999 }))).rejects.toThrow("limit");
        await expect(handleHarnessRequest(request("ui.open", { kind: "preview" }))).rejects.toThrow("agent session");
    });
});
