import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useEffect, useRef } from "react";
import { performanceTelemetry } from "../lib/performance";
import { dispatchPaths } from "../state/dropRegistry";
import type { PtyContext } from "../state/types";
import { claimWorkbenchItemRuntime, disposeWorkbenchItemRuntime, resetWorkbenchItemRuntimeForTests } from "../workbench/itemRuntime";
import { createItemId } from "../workbench/registry";
import { resetPtyShellSubscriptionsForTests, type PtyShellMetadataEvent } from "../api/ptyShell";
import { taskPtyBindings, type TaskPtyBinding } from "../tasks/nativeRuntime";
import type { NativePtyController } from "./usePty";
import { encodePosixShellLiteral, encodePowerShellLiteral, ptyResourceFingerprint, shellSemanticsForExecutable, usePty } from "./usePty";

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
    taskPtyBindings.reset();
    await resetWorkbenchItemRuntimeForTests();
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

function Harness({
    context,
    initialDropPaths,
    initialInput,
    onInitialInputDelivered,
    durable = false,
    cwd = "/repo",
    startup = "codex",
    onShellMetadata,
    externallyOwned = false,
    onController,
}: {
    context: PtyContext;
    initialDropPaths?: readonly string[];
    initialInput?: string;
    onInitialInputDelivered?: () => void;
    durable?: boolean;
    cwd?: string;
    startup?: string;
    onShellMetadata?: (event: PtyShellMetadataEvent) => void;
    externallyOwned?: boolean;
    onController?: (controller: NativePtyController | null) => void;
}) {
    const hostRef = useRef<HTMLDivElement>(null);
    const controllerRef = usePty({
        cwd,
        startup,
        initialDropPaths,
        initialInput,
        onInitialInputDelivered,
        hostRef,
        context,
        onShellMetadata,
        externallyOwned,
        durableItemId: durable ? context.paneId : undefined,
    });
    useEffect(() => {
        onController?.(controllerRef.current);
    }, [controllerRef, onController]);
    return <div ref={hostRef} />;
}

describe("terminal path literal encoding", () => {
    it("preserves POSIX metacharacters inside a single-quoted literal", () => {
        const path = "-literal path\n$(touch nope)`whoami`;pipe|glob*?[x]>out<in\\slash'quote";

        expect(encodePosixShellLiteral(path)).toBe("'-literal path\n$(touch nope)`whoami`;pipe|glob*?[x]>out<in\\slash'\\''quote'");
    });

    it("preserves PowerShell metacharacters and doubles embedded single quotes", () => {
        const path = "C:\\O'Brien\\-literal path\n$env:TEMP\\$(x)`x;pipe|glob*?>out<in";

        expect(encodePowerShellLiteral(path)).toBe("'C:\\O''Brien\\-literal path\n$env:TEMP\\$(x)`x;pipe|glob*?>out<in'");
    });

    it("rejects NUL in both shell encoders", () => {
        expect(() => encodePosixShellLiteral("safe\0unsafe")).toThrow(/NUL/);
        expect(() => encodePowerShellLiteral("safe\0unsafe")).toThrow(/NUL/);
    });

    it("selects semantics from the configured executable rather than its host platform", () => {
        expect(shellSemanticsForExecutable("/bin/zsh")).toBe("posix");
        expect(shellSemanticsForExecutable("C:\\Program Files\\Git\\bin\\bash.exe")).toBe("posix");
        expect(shellSemanticsForExecutable('"C:\\Program Files\\PowerShell\\7\\pwsh.exe"')).toBe("powershell");
        expect(shellSemanticsForExecutable("/opt/microsoft/powershell/7/PWSH")).toBe("powershell");
        expect(shellSemanticsForExecutable("/bin/custom-shell")).toBeNull();
    });

    it("fingerprints every launch field that can change PTY identity", () => {
        const base = {
            cwd: "/repo",
            startup: "zsh",
            context: { sessionId: "s", sessionName: "repo", sessionKind: "project" as const, paneId: "pane" },
        };
        expect(ptyResourceFingerprint(base)).toBe(ptyResourceFingerprint({ ...base }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, cwd: "/other" }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, startup: "bash" }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, initialDropPaths: ["/tmp/image.png"] }));
        expect(ptyResourceFingerprint(base)).not.toBe(ptyResourceFingerprint({ ...base, context: { ...base.context, sessionName: "renamed" } }));

        const taskBinding = Object.freeze({
            paneId: createItemId("pane"),
            ptyId: 70,
            executionId: "execution-1",
            terminalKey: "task",
            revision: 1,
        }) satisfies TaskPtyBinding;
        const external = { ...base, externallyOwned: true };
        expect(ptyResourceFingerprint(external, taskBinding)).not.toBe(
            ptyResourceFingerprint(external, { ...taskBinding, executionId: "execution-2" }),
        );
        expect(ptyResourceFingerprint(external, taskBinding)).not.toBe(ptyResourceFingerprint(external, { ...taskBinding, revision: 2 }));
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
        claimWorkbenchItemRuntime(createItemId(context.paneId!));
        const onController = vi.fn();

        render(<Harness context={context} durable externallyOwned onController={onController} />);
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
        claimWorkbenchItemRuntime(createItemId(context.paneId!));
        const controllers: Array<NativePtyController | null> = [];
        render(<Harness context={context} durable externallyOwned onController={(controller) => controllers.push(controller)} />);
        await waitFor(() => expect(controllers.at(-1)).toBeNull());

        act(() => {
            taskPtyBindings.bind(context.paneId!, { ptyId: 81, executionId: "execution-1", terminalKey: "task" });
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
        const runtimeLease = claimWorkbenchItemRuntime(createItemId(context.paneId!));
        taskPtyBindings.bind(context.paneId!, { ptyId: 91, executionId: "execution-1", terminalKey: "task" });
        const controllers: NativePtyController[] = [];
        render(
            <Harness
                context={context}
                durable
                externallyOwned
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
            taskPtyBindings.bind(context.paneId!, { ptyId: 92, executionId: "execution-2", terminalKey: "task" });
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

        await disposeWorkbenchItemRuntime(runtimeLease);
        expect(invoke).toHaveBeenCalledWith("pty_unsubscribe", { id: 92, subId: 2 });
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
        taskPtyBindings.bind(context.paneId!, { ptyId: 101, executionId: "execution-1", terminalKey: "task" });
        let controller: NativePtyController | null = null;
        const view = render(<Harness context={context} externallyOwned onController={(value) => (controller = value)} />);
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
        taskPtyBindings.bind(context.paneId!, { ptyId: 111, executionId: "execution-1", terminalKey: "task" });
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
            taskPtyBindings.bind(context.paneId!, { ptyId: 112, executionId: "execution-2", terminalKey: "task" });
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

    it("keeps a pane PTY alive across renderer remounts until item disposal", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-durable",
        };
        const runtimeLease = claimWorkbenchItemRuntime(createItemId(context.paneId!));
        const first = render(<Harness context={context} durable />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));
        first.unmount();
        expect(invoke.mock.calls.some(([command]) => command === "pty_kill")).toBe(false);

        const second = render(<Harness context={context} durable />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));
        second.unmount();
        await disposeWorkbenchItemRuntime(runtimeLease);
        expect(invoke.mock.calls.filter(([command]) => command === "pty_kill")).toHaveLength(1);
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
        const runtimeLease = claimWorkbenchItemRuntime(createItemId(context.paneId!));
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

        await disposeWorkbenchItemRuntime(runtimeLease);
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
        const view = render(<Harness context={context} initialInput="one shot" />);
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1));

        view.rerender(<Harness context={{ ...context, initialPromptSubmitted: true }} />);
        await Promise.resolve();

        expect(invoke.mock.calls.filter(([command]) => command === "pty_spawn")).toHaveLength(1);
        expect(invoke.mock.calls.filter(([command]) => command === "pty_kill")).toHaveLength(0);
    });

    it("refuses a late durable renderer after its ownership generation closes", async () => {
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
            windowId: "window-1",
            paneId: "pane-retired",
        };
        const runtimeLease = claimWorkbenchItemRuntime(createItemId(context.paneId!));
        await disposeWorkbenchItemRuntime(runtimeLease);

        render(<Harness context={context} durable />);
        await Promise.resolve();

        expect(invoke.mock.calls.some(([command]) => command === "pty_spawn")).toBe(false);
        expect(performanceTelemetry.snapshot().counters["terminal.durable-owner-missing"]).toBe(1);
    });

    it("quotes every dropped path with the semantics of the configured POSIX shell", async () => {
        invoke.mockImplementation(async (command: string) => {
            if (command === "pty_spawn") return 42;
            if (command === "integration_health") return { shell: "C:\\Program Files\\Git\\bin\\bash.exe" };
            return null;
        });
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
        };
        const view = render(<Harness context={context} />);
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_spawn", expect.anything()));

        const host = view.container.firstElementChild as HTMLElement;
        expect(dispatchPaths(host, ["/tmp/a b", "/tmp/O'Brien", "-$(touch nope);|*?>\nfile"])).toBe(true);

        await waitFor(() => {
            expect(invoke).toHaveBeenCalledWith("pty_write", {
                id: 42,
                data: "\x1b[200~'/tmp/a b' '/tmp/O'\\''Brien' '-$(touch nope);|*?>\nfile'\x1b[201~",
            });
        });
    });

    it("rejects the whole drop when any path contains NUL", async () => {
        const rejected = vi.spyOn(performanceTelemetry, "incrementCounter");
        const context: PtyContext = {
            sessionId: "session-1",
            sessionName: "repo",
            sessionKind: "project",
            project: "/repo",
        };
        const view = render(<Harness context={context} />);
        await waitFor(() => expect(invoke).toHaveBeenCalledWith("pty_spawn", expect.anything()));

        const host = view.container.firstElementChild as HTMLElement;
        expect(dispatchPaths(host, ["/tmp/safe", "/tmp/unsafe\0suffix"])).toBe(true);

        expect(invoke.mock.calls.some(([command]) => command === "integration_health")).toBe(false);
        expect(invoke.mock.calls.some(([command]) => command === "pty_write")).toBe(false);
        expect(rejected.mock.calls.filter(([name]) => name === "terminal.drop.rejected.nul")).toHaveLength(1);
    });

    it("reports a failed drop write exactly once", async () => {
        const writeError = new Error("write failed");
        invoke.mockImplementation(async (command: string) => {
            if (command === "pty_spawn") return 42;
            if (command === "integration_health") return { shell: "/bin/zsh" };
            if (command === "pty_write") throw writeError;
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
        await waitFor(() => expect(invoke.mock.calls.some(([command]) => command === "pty_write")).toBe(true));
        await Promise.resolve();
        await Promise.resolve();

        expect(errors.mock.calls.filter(([name]) => name === "terminal.controller.errors.write")).toHaveLength(1);
    });
});
