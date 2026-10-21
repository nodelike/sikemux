import { useCallback, useEffect, useMemo, useRef, useSyncExternalStore, type RefObject } from "react";
import { Channel } from "@tauri-apps/api/core";
import { coreCall } from "../api/core";
import { workspaceSent } from "../state/workspaceSent";
import { invokeCommand as invoke } from "../api/invoke";
import { registerPtyDrop } from "../state/dropRegistry";
import { registerTextInsert } from "../state/textInsertRegistry";
import { IS_WINDOWS } from "../lib/platform";
import type { PtyContext, PtyDirectCommand } from "../state/types";
import { paneResource } from "./paneResources";
import { PtyLifecycleController, type PtyApi, type PtyAttachResult, type PtyChannelAdapter, type PtyControllerErrorEvent } from "./ptyController";
import { performanceTelemetry } from "../lib/performance";
import { subscribePtyShellMetadata, type PtyShellMetadataEvent } from "../api/ptyShell";
import { runShownIn, taskRuns, type TaskRun } from "../tasks/coreTasks";
import { coreSessionsApi } from "../api/coreSessions";
import { noteSpawnedSession, takeResumableSession } from "./sessionResume";
import { noteTerminalSize } from "./spareShell";
import { integrationHealth } from "../api/system";

type NativeChannel = Channel<ArrayBuffer>;
export type NativePtyController = PtyLifecycleController<NativeChannel, PtyContext>;
export type TerminalShellSemantics = "posix" | "powershell";

const DEFAULT_SHELL_SEMANTICS: TerminalShellSemantics = IS_WINDOWS ? "powershell" : "posix";

interface PtyResourceConfiguration {
    readonly cwd?: string;
    readonly startup?: string;
    readonly directCommand?: PtyDirectCommand;
    readonly initialDropPaths?: readonly string[];
    readonly initialInput?: string;
    readonly initialDraft?: string;
    readonly context?: PtyContext;
    readonly taskTerminalKey?: string;
}

const NOOP = () => {};

/** The task process a task terminal shows: the newest run behind its key. */
interface TaskPtyBinding {
    readonly ptyId: number;
    readonly executionId: string;
    readonly terminalKey: string;
}

function bindingOf(run: TaskRun | null): TaskPtyBinding | null {
    return run?.ptyId === undefined ? null : { ptyId: run.ptyId, executionId: run.executionId, terminalKey: run.terminalKey };
}

function createExternalControllerRef(binding: TaskPtyBinding | null): RefObject<NativePtyController | null> {
    // A fresh ref identity makes useXterm restart for each immutable binding.
    void binding;
    return { current: null };
}

const ATTACH_HEADER_PREFIX_BYTES = 4;
const attachHeaderDecoder = new TextDecoder();

/** `[header length as 4 little-endian bytes][header JSON][replay bytes]`. */
function decodeAttachResponse(body: ArrayBuffer): PtyAttachResult {
    if (body.byteLength < ATTACH_HEADER_PREFIX_BYTES) throw new TypeError("PTY attach returned a truncated response");
    const headerBytes = new DataView(body).getUint32(0, true);
    if (headerBytes > body.byteLength - ATTACH_HEADER_PREFIX_BYTES) throw new TypeError("PTY attach returned a truncated response");
    const headerEnd = ATTACH_HEADER_PREFIX_BYTES + headerBytes;
    const header: unknown = JSON.parse(attachHeaderDecoder.decode(new Uint8Array(body, ATTACH_HEADER_PREFIX_BYTES, headerBytes)));
    if (typeof header !== "object" || header === null) throw new TypeError("PTY attach returned an invalid header");
    const { subId, alternateScreen, shell } = header as Partial<PtyAttachResult>;
    // The controller validates every field; the view shares the response buffer.
    return { subId: subId as number, alternateScreen: alternateScreen as boolean, shell, snapshot: new Uint8Array(body, headerEnd) };
}

/** The same terminal commands, for a terminal on `host`, another host the app joined; the core there names it. */
function remotePtyApi(host: string): PtyApi<NativeChannel, PtyContext> {
    return {
        ...nativePtyApi,
        spawn: async (request) => {
            const id = await invoke<number>("pty_spawn", { ...request, host });
            noteSpawnedSession(id);
            return id;
        },
        resize: (id, cols, rows) => invoke<void>("pty_resize", { id, cols, rows }),
        resume: async (id) => takeResumableSession(id) && (await coreSessionsApi.list(host)).some((session) => session.id === id),
    };
}

const nativePtyApi: PtyApi<NativeChannel, PtyContext> = {
    /* A terminal agent's command line is the core's, built from the agent it keeps. */
    spawn: async (request) => {
        const agentId = request.context?.agentId;
        const id =
            agentId && !request.directCommand && !request.startup
                ? await workspaceSent().then(
                      async () =>
                          (await coreCall<{ ptyId: number }>("startTerminalAgent", { agentId, cols: request.cols, rows: request.rows })).ptyId,
                  )
                : await invoke<number>("pty_spawn", { ...request });
        noteSpawnedSession(id);
        return id;
    },
    write: (id, data) => invoke<void>("pty_write", { id, data }),
    resize: (id, cols, rows) => {
        noteTerminalSize(cols, rows);
        return invoke<void>("pty_resize", { id, cols, rows });
    },
    kill: coreSessionsApi.kill,
    attach: async (id, channel) => decodeAttachResponse(await invoke<ArrayBuffer>("pty_attach", { id, onEvent: channel })),
    detach: (id, subId) => invoke<void>("pty_unsubscribe", { id, subId }),
    ack: (id, subId, bytes) => invoke<void>("pty_ack", { id, subId, bytes }),
    resume: async (id) => takeResumableSession(id) && (await coreSessionsApi.list()).some((session) => session.id === id),
};

const nativeChannels: PtyChannelAdapter<NativeChannel> = {
    create: (onMessage) => {
        const channel = new Channel<ArrayBuffer>();
        // Native output arrives as raw IPC bytes; the view shares the buffer.
        channel.onmessage = (buffer) => onMessage(new Uint8Array(buffer));
        return {
            transport: channel,
            close: () => {
                channel.onmessage = () => {};
            },
        };
    },
};

function requireLiteralPath(path: string): void {
    if (path.includes("\0")) throw new TypeError("terminal drop paths cannot contain NUL bytes");
}

/** Encode one path as a single POSIX shell word without evaluating any of it. */
export function encodePosixShellLiteral(path: string): string {
    requireLiteralPath(path);
    return `'${path.replaceAll("'", "'\\''")}'`;
}

/** Encode one path as a PowerShell single-quoted string literal. */
export function encodePowerShellLiteral(path: string): string {
    requireLiteralPath(path);
    return `'${path.replaceAll("'", "''")}'`;
}

/** Determine quoting rules from the configured executable, independent of host OS. */
export function shellSemanticsForExecutable(shell: string): TerminalShellSemantics | null {
    const trimmed = shell.trim();
    const unquoted =
        trimmed.length >= 2 && ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'")))
            ? trimmed.slice(1, -1)
            : trimmed;
    const executable =
        unquoted
            .split(/[\\/]/)
            .at(-1)
            ?.toLowerCase()
            .replace(/\.exe$/, "") ?? "";
    if (executable === "powershell" || executable === "pwsh") return "powershell";
    if (["sh", "bash", "zsh", "dash", "ash", "ksh", "mksh", "fish"].includes(executable)) return "posix";
    return null;
}

async function configuredShellSemantics(): Promise<TerminalShellSemantics> {
    try {
        const { shell } = await integrationHealth();
        return shellSemanticsForExecutable(shell) ?? DEFAULT_SHELL_SEMANTICS;
    } catch {
        // invokeCommand already records the failed IPC. Keep drag/drop usable
        // with the same default the native PTY launcher uses on this platform.
    }
    return DEFAULT_SHELL_SEMANTICS;
}

function encodeDroppedPaths(paths: readonly string[], semantics: TerminalShellSemantics): string {
    const encode = semantics === "powershell" ? encodePowerShellLiteral : encodePosixShellLiteral;
    return paths.map(encode).join(" ");
}

function recordControllerError(event: PtyControllerErrorEvent): void {
    performanceTelemetry.incrementCounter(`terminal.controller.errors.${event.operation}`);
}

/** Exact, content-local identity used only to prevent stale PTY reuse. */
export function ptyResourceFingerprint(configuration: PtyResourceConfiguration, taskBinding: TaskPtyBinding | null = null): string {
    const context = configuration.context;
    return JSON.stringify([
        configuration.cwd ?? null,
        configuration.startup ?? null,
        configuration.directCommand?.program ?? null,
        configuration.directCommand?.args ?? null,
        configuration.directCommand?.profile?.configPath ?? null,
        configuration.directCommand?.profile?.environmentKeys ?? null,
        configuration.initialDropPaths ?? null,
        configuration.initialInput ?? null,
        configuration.initialDraft ?? null,
        context?.sessionId ?? null,
        context?.sessionName ?? null,
        context?.sessionKind ?? null,
        context?.project ?? null,
        context?.windowId ?? null,
        context?.paneId ?? null,
        context?.agentId ?? null,
        context?.agentType ?? null,
        context?.initialPromptSubmitted ?? null,
        context?.shellIntegration ?? null,
        configuration.taskTerminalKey ?? null,
        taskBinding?.ptyId ?? null,
        taskBinding?.executionId ?? null,
        taskBinding?.terminalKey ?? null,
    ]);
}

export function usePty(opts: {
    cwd?: string;
    startup?: string;
    directCommand?: PtyDirectCommand;
    /** Native file drops replayed as independent paste events before the first task. */
    initialDropPaths?: readonly string[];
    initialInput?: string;
    /** Pasted once the program starts and never submitted. */
    initialDraft?: string;
    onInitialInputDelivered?: () => void;
    hostRef: RefObject<HTMLDivElement | null>;
    spawnWhen?: boolean;
    context?: PtyContext;
    onShellMetadata?: (event: PtyShellMetadataEvent) => void;
    /** Show the process of the task run behind this key; never spawn a shell. */
    taskTerminalKey?: string;
    /** Durable workbench item owner. Omit for popups, agents, and embedded shells. */
    durableItemId?: string;
    /** The process this terminal showed before the page loaded; read once, when the controller is made. */
    resumePtyId?: number;
    onPtySession?: (id: number) => void;
    /** Another host the app joined, which runs the terminal. */
    host?: string;
}): RefObject<NativePtyController | null> {
    const { hostRef, spawnWhen = true, taskTerminalKey } = opts;
    const externallyOwned = taskTerminalKey !== undefined;
    const subscribeTaskBinding = useCallback((listener: () => void) => (externallyOwned ? taskRuns.subscribe(listener) : NOOP), [externallyOwned]);
    const taskExecution = useSyncExternalStore(subscribeTaskBinding, () =>
        taskTerminalKey === undefined ? null : (runShownIn(taskRuns.getSnapshot(), taskTerminalKey)?.executionId ?? null),
    );
    const taskBinding = useMemo(
        () => (taskTerminalKey === undefined || taskExecution === null ? null : bindingOf(runShownIn(taskRuns.getSnapshot(), taskTerminalKey))),
        [taskTerminalKey, taskExecution],
    );
    const ordinaryControllerRef = useRef<NativePtyController | null>(null);
    const externalControllerRef = useMemo(() => createExternalControllerRef(taskBinding), [taskBinding]);
    const controllerRef = externallyOwned ? externalControllerRef : ordinaryControllerRef;
    const deliveredRef = useRef(opts.onInitialInputDelivered);
    deliveredRef.current = opts.onInitialInputDelivered;
    const shellMetadataRef = useRef(opts.onShellMetadata);
    shellMetadataRef.current = opts.onShellMetadata;
    const ptySessionRef = useRef(opts.onPtySession);
    ptySessionRef.current = opts.onPtySession;
    const currentOptionsRef = useRef(opts);
    currentOptionsRef.current = opts;
    const resourceFingerprint = ptyResourceFingerprint(opts, externallyOwned ? taskBinding : null);
    const durableItemId = opts.durableItemId ?? null;
    // Transient agent/popup terminals intentionally keep mount-time launch
    // options: clearing a delivered initial prompt must not respawn the CLI.
    const durableResourceFingerprint = durableItemId ? resourceFingerprint : null;
    // External transient controllers must also rotate with task executions.
    const controllerLifecycleFingerprint = externallyOwned ? resourceFingerprint : durableResourceFingerprint;

    useEffect(() => {
        if (externallyOwned && !taskBinding) {
            controllerRef.current = null;
            if (durableItemId)
                paneResource<NativePtyController | null>(durableItemId, durableResourceFingerprint!, () => ({ value: null, dispose: NOOP }));
            return;
        }
        const initial = currentOptionsRef.current;
        const createController = () =>
            new PtyLifecycleController<NativeChannel, PtyContext>({
                api: initial.host ? remotePtyApi(initial.host) : nativePtyApi,
                channels: nativeChannels,
                existingPtyId: externallyOwned ? taskBinding!.ptyId : undefined,
                resumePtyId: externallyOwned ? undefined : initial.resumePtyId,
                onProcess: (id) => ptySessionRef.current?.(id),
                cwd: initial.cwd,
                startup: initial.startup,
                directCommand: initial.directCommand,
                context: initial.context,
                initialPastes: externallyOwned
                    ? undefined
                    : initial.initialDropPaths?.map((path) => encodeDroppedPaths([path], DEFAULT_SHELL_SEMANTICS)),
                initialInput: externallyOwned ? undefined : initial.initialInput,
                initialDraft: externallyOwned ? undefined : initial.initialDraft,
                onInitialInputDelivered: () => deliveredRef.current?.(),
                onError: recordControllerError,
            });
        const controller = durableItemId
            ? paneResource(durableItemId, durableResourceFingerprint!, () => {
                  const value = createController();
                  return { value, dispose: () => void value.dispose() };
              })
            : createController();
        controllerRef.current = controller;

        const host = hostRef.current;
        let active = true;
        let shellSemanticsPromise: Promise<TerminalShellSemantics> | null = null;
        const resolveShellSemantics = () => (shellSemanticsPromise ??= configuredShellSemantics());
        const unregisterDrop = host
            ? registerPtyDrop(host, (paths) => {
                  if (paths.length === 0) return;
                  const droppedPaths = [...paths];
                  if (droppedPaths.some((path) => path.includes("\0"))) {
                      performanceTelemetry.incrementCounter("terminal.drop.rejected.nul");
                      return;
                  }
                  void resolveShellSemantics()
                      .then((semantics) => {
                          if (!active) return;
                          const body = encodeDroppedPaths(droppedPaths, semantics);
                          return controller.write(`\x1b[200~${body}\x1b[201~`);
                      })
                      // PtyLifecycleController is the sole reporter for write
                      // failures; this catch only prevents an unhandled promise.
                      .catch(() => {});
              })
            : () => {};
        const unregisterTextInsert = host
            ? registerTextInsert(host, (text) => {
                  if (text.includes("\0")) return;
                  void controller.write(`\x1b[200~${text} \x1b[201~`);
              })
            : () => {};

        return () => {
            active = false;
            unregisterDrop();
            unregisterTextInsert();
            if (controllerRef.current === controller) controllerRef.current = null;
            if (!durableItemId) void controller.dispose();
        };
    }, [hostRef, durableItemId, durableResourceFingerprint, controllerLifecycleFingerprint, controllerRef, externallyOwned, taskBinding]);

    useEffect(() => {
        if (!spawnWhen) return;
        const controller = controllerRef.current;
        if (!controller) return;
        let disposed = false;
        let unlisten = () => {};
        void controller
            .start()
            .then(
                (ptyId) => {
                    if (disposed || !currentOptionsRef.current.context?.shellIntegration || !shellMetadataRef.current) return;
                    return subscribePtyShellMetadata(ptyId, (event) => shellMetadataRef.current?.(event));
                },
                () => undefined,
            )
            .then((nextUnlisten) => {
                if (!nextUnlisten) return;
                if (disposed) nextUnlisten();
                else unlisten = nextUnlisten;
            })
            .catch(() => {
                if (!disposed) performanceTelemetry.incrementCounter("terminal.shell-metadata.subscribe-errors");
            });
        return () => {
            disposed = true;
            unlisten();
        };
    }, [spawnWhen, controllerLifecycleFingerprint, controllerRef, opts.context?.shellIntegration]);

    return controllerRef;
}
