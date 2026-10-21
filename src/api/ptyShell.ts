import { isRecord } from "../lib/guards";
import { performanceTelemetry } from "../lib/performance";
import { parsePtyShellMetadataSnapshot, type PtyShellMetadataSnapshot, type PtyShellPhase } from "../terminal/ptyController";
import { coreRequest } from "./core";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";

export type PtyShellBoundary = "cwd" | "prompt_start" | "command_start" | "command_executed" | "command_finished";

export interface PtyShellMetadataEvent {
    readonly ptyId: number;
    readonly revision: number;
    readonly boundary: PtyShellBoundary;
    readonly cwd: string | null;
    readonly phase: PtyShellPhase;
    readonly exitCode: number | null;
}

/** Whether the shell is waiting at its prompt with nothing running in it. */
export const ptyShellAtPrompt = (ptyId: number): Promise<boolean> =>
    coreRequest<{ atPrompt: boolean }>({ op: "shellAtPrompt", id: ptyId }).then(({ atPrompt }) => atPrompt);

export type PtyShellMetadataListener = (event: PtyShellMetadataEvent) => void;

export const PTY_SHELL_METADATA_EVENT = "pty_shell_metadata";
const BOUNDARIES = new Set<PtyShellBoundary>(["cwd", "prompt_start", "command_start", "command_executed", "command_finished"]);
const listeners = new Map<number, Set<PtyShellMetadataListener>>();
let nativeUnlisten: IpcUnsubscribe | null = null;
let nativeListenPromise: Promise<void> | null = null;

export function parsePtyShellMetadataEvent(value: unknown): PtyShellMetadataEvent | null {
    if (
        !isRecord(value) ||
        typeof value.ptyId !== "number" ||
        typeof value.boundary !== "string" ||
        !BOUNDARIES.has(value.boundary as PtyShellBoundary)
    ) {
        return null;
    }
    const shell = parsePtyShellMetadataSnapshot({ revision: value.revision, cwd: value.cwd, phase: value.phase, lastExitCode: null });
    if (!shell) return null;
    const exitCode = value.exitCode ?? null;
    if (exitCode !== null && typeof exitCode !== "number") return null;
    return {
        ptyId: value.ptyId,
        revision: shell.revision,
        boundary: value.boundary as PtyShellBoundary,
        cwd: shell.cwd,
        phase: shell.phase,
        exitCode,
    };
}

/** Apply only a newer event; shell metadata is never an authorization input. */
export function applyPtyShellMetadataEvent(current: PtyShellMetadataSnapshot | null, event: PtyShellMetadataEvent): PtyShellMetadataSnapshot {
    if (current && event.revision <= current.revision) return current;
    return {
        revision: event.revision,
        cwd: event.cwd,
        phase: event.phase,
        lastExitCode: event.boundary === "command_finished" ? event.exitCode : (current?.lastExitCode ?? null),
    };
}

function listenerCount(): number {
    let count = 0;
    for (const group of listeners.values()) count += group.size;
    return count;
}

function dispatch(payload: unknown): void {
    const event = parsePtyShellMetadataEvent(payload);
    if (!event) {
        performanceTelemetry.incrementCounter("terminal.shell-metadata.invalid-events");
        return;
    }
    const group = listeners.get(event.ptyId);
    if (!group) return;
    for (const listener of Array.from(group)) listener(event);
}

function stopNativeListenerIfIdle(): void {
    if (listenerCount() !== 0 || !nativeUnlisten) return;
    const unlisten = nativeUnlisten;
    nativeUnlisten = null;
    unlisten();
}

function ensureNativeListener(): Promise<void> {
    if (nativeUnlisten) return Promise.resolve();
    if (nativeListenPromise) return nativeListenPromise;
    nativeListenPromise = getIpcTransport()
        .subscribe<unknown>(PTY_SHELL_METADATA_EVENT, (event) => dispatch(event.payload))
        .then(
            (unlisten) => {
                nativeListenPromise = null;
                nativeUnlisten = unlisten;
                stopNativeListenerIfIdle();
            },
            (error: unknown) => {
                nativeListenPromise = null;
                throw error;
            },
        );
    void nativeListenPromise.catch(() => {});
    return nativeListenPromise;
}

/** Shares one native listener across all live PTYs and filters by runtime ID. */
export async function subscribePtyShellMetadata(ptyId: number, listener: PtyShellMetadataListener): Promise<IpcUnsubscribe> {
    const group = listeners.get(ptyId) ?? new Set();
    group.add(listener);
    listeners.set(ptyId, group);
    try {
        await ensureNativeListener();
    } catch (error) {
        group.delete(listener);
        if (group.size === 0) listeners.delete(ptyId);
        throw error;
    }

    let active = true;
    return () => {
        if (!active) return;
        active = false;
        const current = listeners.get(ptyId);
        current?.delete(listener);
        if (current?.size === 0) listeners.delete(ptyId);
        stopNativeListenerIfIdle();
    };
}

export async function resetPtyShellSubscriptionsForTests(): Promise<void> {
    listeners.clear();
    if (nativeListenPromise) await nativeListenPromise.catch(() => {});
    stopNativeListenerIfIdle();
}
