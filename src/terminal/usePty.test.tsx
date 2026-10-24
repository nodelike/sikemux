import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useEffect, useRef } from "react";
import { performanceTelemetry } from "../lib/performance";
import { dispatchPaths } from "../state/dropRegistry";
import type { PtyContext } from "../state/types";
import { resetPaneResourcesForTests } from "./paneResources";
import { resetPtyShellSubscriptionsForTests, type PtyShellMetadataEvent } from "../api/ptyShell";
import { setTaskRunsForTests } from "../tasks/coreTasks";
import type { NativePtyController } from "./usePty";
import { ptyResourceFingerprint, usePty } from "./usePty";

function attachResponse(subId: number, snapshot = new Uint8Array()): ArrayBuffer {
    const header = new TextEncoder().encode(JSON.stringify({ subId, alternateScreen: false }));
    const body = new Uint8Array(4 + header.length + snapshot.length);
    new DataView(body.buffer).setUint32(0, header.length, true);
    body.set(header, 4);
    body.set(snapshot, 4 + header.length);
    return body.buffer;
}

const { invoke, listen, unlisten, shellEvent, nativeChannels, attachSequence } = vi.hoisted(() => ({
    invoke: vi.fn(async (command: string) => {
        if (command === "pty_spawn") return 42;
        if (command === "pty_attach") return attachResponse(++attachSequence.current);
        if (command === "integration_health") return { shell: "/bin/zsh" };
        return null;
    }),
    listen: vi.fn(async (_name: string, handler: (event: { payload: unknown }) => void) => {
        shellEvent.current = handler;
        return unlisten;
    }),
    unlisten: vi.fn(),
    shellEvent: { current: null as ((event: { payload: unknown }) => void) | null },
    nativeChannels: [] as Array<{ onmessage: (message: number[]) => void }>,
    attachSequence: { current: 0 },
}));

vi.mock("@tauri-apps/api/core", () => ({
    invoke,
    Channel: class TestChannel {
        onmessage = (_message: number[]) => {};

        constructor() {
            nativeChannels.push(this);
        }
    },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

afterEach(async () => {
    cleanup();
    setTaskRunsForTests([]);
    resetPaneResourcesForTests();
    await resetPtyShellSubscriptionsForTests();
    vi.restoreAllMocks();
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) => {
        if (command === "pty_spawn") return 42;
        if (command === "pty_attach") return attachResponse(++attachSequence.current);
        if (command === "integration_health") return { shell: "/bin/zsh" };
        return null;
    });
    listen.mockClear();
    unlisten.mockClear();
    shellEvent.current = null;
    nativeChannels.length = 0;
    attachSequence.current = 0;
});

function run(ptyId: number, executionId: string) {
    return { executionId, taskId: "task", project: "/repo", status: "running" as const, ptyId, agentId: null, terminalKey: "task" };
}

function Harness({
    context,
    initialDraft,
    onInitialInputDelivered,
    durable = false,
    cwd = "/repo",
    startup = "codex",
    onShellMetadata,
    taskTerminalKey,
    onController,
}: {
    context: PtyContext;
    initialDraft?: string;
    onInitialInputDelivered?: () => void;
    durable?: boolean;
    cwd?: string;
    startup?: string;
    onShellMetadata?: (event: PtyShellMetadataEvent) => void;
    taskTerminalKey?: string;
    onController?: (controller: NativePtyController | null) => void;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const controllerRef = usePty({
        cwd,
        startup,
        initialDraft,
        onInitialInputDelivered,
        hostRef,
        context,
        onShellMetadata,
        taskTerminalKey,
        durableItemId: durable ? context.paneId : undefined,
    });
    useEffect(() => {
        onController?.(controllerRef.current);
    }, [controllerRef, onController]);
    return <div ref={hostRef} />;
}

describe("PTY resource identity", () => {
    it("fingerprints every launch field that can change PTY identity", () => {
        const base = {
            cwd: "/repo",
            startup: "zsh",
            context: { sessionId: "s", sessionName: "repo", sessionKind: "project" as const, paneId: "pane" },
        };
        expect(ptyResourceFingerprint(base)).toBe(ptyResourceFingerprint({ ...base }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, cwd: "/other" }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, startup: "bash" }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, context: { ...base.context, sessionName: "renamed" } }));

        const taskBinding = Object.freeze({ ptyId: 70, executionId: "execution-1", terminalKey: "task" });
        const external = { ...base, taskTerminalKey: "task" };
        expect(ptyResourceFingerprint(external, taskBinding)).not.toBe(
            ptyResourceFingerprint(external, { ...taskBinding, executionId: "execution-2" }),
        );
    });
});

describe("usePty", () => {
    it("sends the complete typed terminal identity to the backend", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-1",
            agentId: "agent-1",
            agentType: "codex",
        };

        render(<Harness context={context} />);

        await waitFor(() => {
            expect(invoke).toHaveBeenCalledWith("pty_spawn", {
                cols: 80,
                rows: 24,
                cwd: "/repo",
                startup: "codex",
                directCommand: null,
                context,
            });
        });
    });

    it("keeps an opted-in task pane controller-free while no binding exists", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-task-unbound",
        };
        const onController = vi.fn();

        render(<Harness context={context} durable taskTerminalKey="task" onController={onController} />);
        await waitFor(() => expect(onController).toHaveBeenLastCalledWith(null));

        expect(invoke.mock.calls.some(([command]) => command === "pty_spawn")).toBe(false);
        expect(invoke.mock.calls.some(([command]) => command === "pty_attach")).toBe(false);
    });

    it("creates the exact externally owned controller when a binding arrives", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-task-arrival",
        };
        const controllers: Array<NativePtyController | null> = [];
        render(<Harness context={context} durable taskTerminalKey="task" onController={(controller) => controllers.push(controller)} />);
        await waitFor(() => expect(controllers.at(-1)).toBeNull());

        act(() => {
            setTaskRunsForTests([run(81, "execution-1")]);
        });
        await waitFor(() => expect(controllers.at(-1)).not.toBeNull());
        const controller = controllers.at(-1)!;
        await expect(controller.start()).resolves.toBe(81);
        const attachment = await controller.attach(vi.fn());

        expect(controller.getSnapshot()).toMatchObject({ status: "running", processOwnership: "external", spawnAttempts: 0 });
        expect(invoke.mock.calls.some(([command]) => command === "pty_spawn")).toBe(false);
        expect(invoke).toHaveBeenCalledWith("pty_attach", { id: 81, onEvent: nativeChannels[0] });

        await attachment.detach();
        expect(invoke).toHaveBeenCalledWith("pty_unsubscribe", { id: 81, subId: 1 });
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);
    });

    it("replaces a durable task binding and detaches both external controllers without killing", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-task-replacement",
        };
        setTaskRunsForTests([run(91, "execution-1")]);
        const controllers: NativePtyController[] = [];
        render(
            <Harness
                context={context}
                durable
                taskTerminalKey="task"
                onController={(controller) => {
                    if (controller) controllers.push(controller);
                }}
            />,
        );
        await waitFor(() => expect(controllers).toHaveLength(1));
        const first = controllers[0]!;
        await first.attach(vi.fn());
        expect(invoke).toHaveBeenCalledWith("pty_attach", { id: 91, onEvent: nativeChannels[0] });

        act(() => {
            setTaskRunsForTests([run(92, "execution-2")]);
        });
        await waitFor(() => expect(controllers).toHaveLength(2));
        const replacement = controllers[1]!;
        expect(replacement).not.toBe(first);
        await expect(replacement.start()).resolves.toBe(92);
        await replacement.attach(vi.fn());

        await waitFor(() => expect(first.getSnapshot().status).toBe("disposed"));
        expect(invoke).toHaveBeenCalledWith("pty_unsubscribe", { id: 91, subId: 1 });
        expect(invoke).toHaveBeenCalledWith("pty_attach", { id: 92, onEvent: nativeChannels[1] });
        expect(invoke.mock.calls.some(([command]) => command === "pty_spawn")).toBe(false);
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);

        resetPaneResourcesForTests();
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_unsubscribe", { id: 92, subId: 2 }));
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);
    });

    it("detaches a transient externally owned controller on unmount without killing", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-task-transient",
        };
        setTaskRunsForTests([run(101, "execution-1")]);
        let controller: NativePtyController | null = null;
        const view = render(<Harness context={context} taskTerminalKey="task" onController={(value) => (controller = value)} />);
        await waitFor(() => expect(controller).not.toBeNull());
        await controller!.attach(vi.fn());

        view.unmount();

        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_unsubscribe", { id: 101, subId: 1 }));
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);
    });

    it("ordinary terminals ignore accidental task bindings and keep spawning shells", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-ordinary-with-binding",
        };
        setTaskRunsForTests([run(111, "execution-1")]);
        const controllers: NativePtyController[] = [];
        render(
            <Harness
                context={context}
                onController={(controller) => {
                    if (controller) controllers.push(controller);
                }}
            />,
        );
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));
        expect(controllers).toHaveLength(1);
        expect(controllers[0]!.getSnapshot().processOwnership).toBe("controller");

        act(() => {
            setTaskRunsForTests([run(112, "execution-2")]);
        });
        await Promise.resolve();

        expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1);
        const calls = invoke.mock.calls as unknown as Array<[string, { id?: number }?]>;
        expect(calls.some(([command, args]) => command === "pty_attach" && (args?.id ?? -1) >= 111)).toBe(false);
        expect(controllers).toHaveLength(1);
    });

    it("subscribes only opted-in live shells and forwards typed metadata", async () => {
        const onShellMetadata = vi.fn();
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-1",
            shellIntegration: true,
        };
        const view = render(<Harness context={context} onShellMetadata={onShellMetadata} />);
        await waitFor(() => expect(listen).toHaveBeenCalledWith("pty_shell_metadata", expect.any(Function)));

        act(() => {
            shellEvent.current?.({
                payload: { ptyId: 42, revision: 1, boundary: "prompt_start", cwd: "/repo", phase: "prompt" },
            });
        });
        expect(onShellMetadata).toHaveBeenCalledWith({
            ptyId: 42,
            revision: 1,
            boundary: "prompt_start",
            cwd: "/repo",
            phase: "prompt",
            exitCode: null,
        });

        view.unmount();
        expect(unlisten).toHaveBeenCalledOnce();
    });

    it("keeps a pane PTY alive across renderer remounts until the pane closes", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-durable",
        };
        const first = render(<Harness context={context} durable />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));
        first.unmount();
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);

        const second = render(<Harness context={context} durable />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));
        second.unmount();
        resetPaneResourcesForTests();
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_kill")).toHaveLength(1));
    });

    it("replaces a durable PTY instead of reusing stale launch configuration", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-reconfigured",
        };
        const view = render(<Harness context={context} durable cwd="/repo/old" startup="zsh" />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));

        view.rerender(<Harness context={context} durable cwd="/repo/new" startup="bash" />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(2));
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_kill")).toHaveLength(1));
        const spawnCalls = invoke.mock.calls.filter(([command]) => command === "pty_spawn") as unknown as Array<[string, unknown]>;
        expect(spawnCalls.at(-1)?.[1]).toMatchObject({
            cwd: "/repo/new",
            startup: "bash",
        });
    });

    it("does not respawn a transient CLI when delivered initial input is cleared", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            agentId: "agent-1",
            agentType: "hermes",
        };
        const view = render(<Harness context={context} initialDraft="one shot" />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));

        view.rerender(<Harness context={{ ...context, initialPromptSubmitted: true }} />);
        await Promise.resolve();

        expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1);
        expect(invoke.mock.calls.filter(([command]) => command === "pty_kill")).toHaveLength(0);
    });

    it("hands dropped paths to the core as they are", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
        };
        const view = render(<Harness context={context} />);
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_spawn", expect.anything()));

        const host = view.container.firstElementChild as HTMLElement;
        expect(dispatchPaths(host, ["/tmp/a b", "/tmp/O'Brien"])).toBe(true);

        await waitFor(() => {
            expect(invoke).toHaveBeenCalledWith("pty_input", { id: 42, input: { type: "paths", paths: ["/tmp/a b", "/tmp/O'Brien"] } });
        });
    });

    it("reports a failed drop exactly once", async () => {
        const writeError = new Error("input failed");
        invoke.mockImplementation(async (command: string) => {
            if (command === "pty_spawn") return 42;
            if (command === "pty_input") throw writeError;
            return null;
        });
        const errors = vi.spyOn(performanceTelemetry, "incrementCounter");
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
        };
        const view = render(<Harness context={context} />);
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_spawn", expect.anything()));

        const host = view.container.firstElementChild as HTMLElement;
        expect(dispatchPaths(host, ["/tmp/file"])).toBe(true);
        await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === "pty_input")).toBe(true));
        await Promise.resolve();
        await Promise.resolve();

        expect(errors.mock.calls.filter(([name]) => name === "terminal.controller.errors.input")).toHaveLength(1);
    });
});
