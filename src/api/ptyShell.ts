import { performanceTelemetry } from "../lib/performance";
import { parsePtyShellMetadataSnapshot, type PtyShellMetadataSnapshot, type PtyShellPhase } from "../terminal/ptyController";
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

export type PtyShellMetadataListener = (event: PtyShellMetadataEvent) => void;

export const PTY_SHELL_METADATA_EVENT = "pty_shell_metadata";
export const PTY_SHELL_SUBSCRIPTION_LIMITS = Object.freeze({ maxPtys: 512, maxListenersPerPty: 8 });

const EVENT_KEYS = new Set(["ptyId", "revision", "boundary", "cwd", "phase", "exitCode"]);
const BOUNDARIES = new Set<PtyShellBoundary>(["cwd", "prompt_start", "command_start", "command_executed", "command_finished"]);
const listeners = new Map<number, Set<PtyShellMetadataListener>>();
let nativeUnlisten: IpcUnsubscribe | null = null;
let nativeListenPromise: Promise<void> | null = null;

function ownDataRecord(value: unknown): Record<string, unknown> | null {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
    try {
        const descriptors = Object.getOwnPropertyDescriptors(value);
        const record: Record<string, unknown> = {};
        for (const key of Reflect.ownKeys(descriptors)) {
            if (typeof key !== "string" || !EVENT_KEYS.has(key)) return null;
            const descriptor = descriptors[key];
            if (!descriptor || !("value" in descriptor)) return null;
            record[key] = descriptor.value;
        }
        return record;
    } catch {
        return null;
    }
}

function validPtyId(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0;
}

function validExitCode(value: unknown): value is number {
    return Number.isInteger(value) && (value as number) >= -2_147_483_648 && (value as number) <= 2_147_483_647;
}

export function parsePtyShellMetadataEvent(value: unknown): PtyShellMetadataEvent | null {
    const record = ownDataRecord(value);
    if (!record || !validPtyId(record.ptyId) || typeof record.boundary !== "string" || !BOUNDARIES.has(record.boundary as PtyShellBoundary)) {
        return null;
    }
    const shell = parsePtyShellMetadataSnapshot({
        revision: record.revision,
        cwd: record.cwd,
        phase: record.phase,
        lastExitCode: null,
    });
    if (!shell) return null;
    const exitCode = record.exitCode === undefined ? null : record.exitCode;
    if (exitCode !== null && !validExitCode(exitCode)) return null;
    if (record.boundary !== "command_finished" && record.exitCode !== undefined) return null;
    return Object.freeze({
        ptyId: record.ptyId,
        revision: shell.revision,
        boundary: record.boundary as PtyShellBoundary,
        cwd: shell.cwd,
        phase: shell.phase,
        exitCode: exitCode as number | null,
    });
}

/** Apply only a newer event; shell metadata is never an authorization input. */
export function applyPtyShellMetadataEvent(current: PtyShellMetadataSnapshot | null, event: PtyShellMetadataEvent): PtyShellMetadataSnapshot {
    if (current && event.revision <= current.revision) return current;
    return Object.freeze({
        revision: event.revision,
        cwd: event.cwd,
        phase: event.phase,
        lastExitCode: event.boundary === "command_finished" ? event.exitCode : (current?.lastExitCode ?? null),
    });
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
    for (const listener of Array.from(group)) {
        try {
            listener(event);
        } catch {
            performanceTelemetry.incrementCounter("terminal.shell-metadata.listener-errors");
        }
    }
}

function stopNativeListenerIfIdle(): void {
    if (listenerCount() !== 0 || !nativeUnlisten) return;
    const unlisten = nativeUnlisten;
    nativeUnlisten = null;
    try {
        unlisten();
    } catch {
        performanceTelemetry.incrementCounter("terminal.shell-metadata.unlisten-errors");
    }
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
    if (!validPtyId(ptyId)) throw new TypeError("PTY shell subscription requires a valid runtime ID");
    if (typeof listener !== "function") throw new TypeError("PTY shell subscription requires a listener");
    let group = listeners.get(ptyId);
    if (!group && listeners.size >= PTY_SHELL_SUBSCRIPTION_LIMITS.maxPtys) throw new RangeError("PTY shell subscription PTY limit reached");
    group ??= new Set();
    if (group.size >= PTY_SHELL_SUBSCRIPTION_LIMITS.maxListenersPerPty) throw new RangeError("PTY shell listener limit reached");
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
