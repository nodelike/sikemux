import type { PtyDirectCommand } from "../state/types";
import type { TerminalInput } from "./grid";
import { errorCategory, errorText } from "@shared/lib/format";
import { isRecord } from "@shared/lib/guards";
import { swallow } from "../state/toast";

export type PtyOutputChunk = readonly number[] | Uint8Array;
export type PtyShellPhase = "unknown" | "prompt" | "input" | "running" | "finished";

export interface PtyShellMetadataSnapshot {
    readonly revision: number;
    readonly cwd: string | null;
    readonly phase: PtyShellPhase;
    readonly lastExitCode: number | null;
}

export interface PtySpawnRequest<Context = unknown> {
    readonly cols: number;
    readonly rows: number;
    readonly cwd: string | null;
    readonly startup: string | null;
    readonly directCommand: PtyDirectCommand | null;
    readonly context: Context | null;
}

export interface PtyAttachResult {
    readonly subId: number;
    /** The stream's first frame, holding the whole screen. */
    readonly frame: Uint8Array;
    readonly shell?: PtyShellMetadataSnapshot | null;
}

export interface PtyApi<ChannelTransport, Context = unknown> {
    spawn(request: PtySpawnRequest<Context>): Promise<number>;
    write(id: number, data: string): Promise<void>;
    /** A key, text, paste, mouse or focus change, which the core encodes for the program. */
    input(id: number, input: TerminalInput): Promise<void>;
    /** A draft left unsent at the program's prompt, which the core types once the program is ready for it. */
    typeWhenReady(id: number, draft: string): Promise<void>;
    resize(id: number, cols: number, rows: number): Promise<void>;
    kill(id: number): Promise<void>;
    attach(id: number, channel: ChannelTransport): Promise<PtyAttachResult>;
    detach(id: number, subId: number): Promise<void>;
    /** Reports the last frame this subscription has drawn. */
    ack(id: number, subId: number, frame: number): Promise<void>;
    /** Whether a process this page did not start may be taken over instead of spawning one. */
    resume?(id: number): Promise<boolean>;
}

export interface PtyChannelBinding<ChannelTransport> {
    readonly transport: ChannelTransport;
    close(): void;
}

export interface PtyChannelAdapter<ChannelTransport> {
    create(onMessage: (chunk: PtyOutputChunk) => void): PtyChannelBinding<ChannelTransport>;
}

export type PtyControllerStatus = "idle" | "starting" | "running" | "failed" | "exited" | "disposing" | "disposed";
export type PtyInitialInputStatus = "none" | "pending" | "delivering" | "delivered" | "failed" | "cancelled";
export type PtyProcessOwnership = "controller" | "external";
export type PtyOperation =
    | "spawn"
    | "initial-input"
    | "initial-input-callback"
    | "write"
    | "input"
    | "resize"
    | "attach"
    | "ack"
    | "detach"
    | "kill"
    | "exit"
    | "output-listener"
    | "state-listener"
    | "channel-close";

export interface PtyControllerSnapshot {
    readonly revision: number;
    readonly status: PtyControllerStatus;
    readonly spawnAttempts: number;
    readonly attachmentCount: number;
    readonly initialInput: PtyInitialInputStatus;
    readonly cols: number;
    readonly rows: number;
    readonly failureOperation: PtyOperation | null;
    readonly processOwnership: PtyProcessOwnership;
}

export interface PtyControllerErrorEvent {
    readonly operation: PtyOperation;
    readonly error: unknown;
}

export interface PtyAttachment {
    /** The stream's first frame, with the whole screen. Apply this before calling activate(). */
    readonly frame: Uint8Array;
    /** Untrusted display hint parsed from opt-in shell integration. */
    readonly shell: PtyShellMetadataSnapshot | null;
    /** Release the frames that came after the first to the renderer in exact order. */
    activate(): void;
    /**
     * Report the last frame drawn. The core sends a stream only a couple of
     * frames ahead of what was drawn; a renderer that falls behind gets the
     * latest screen when it catches up, never a backlog.
     */
    ack(frame: number): void;
    /** Drop only this renderer subscription; the headless PTY keeps running. */
    detach(): Promise<void>;
    toJSON(): never;
}

export interface PtyLifecycleControllerOptions<ChannelTransport, Context = unknown> {
    readonly api: PtyApi<ChannelTransport, Context>;
    readonly channels: PtyChannelAdapter<ChannelTransport>;
    /** Existing native PTY retained and stopped by another runtime owner. */
    readonly existingPtyId?: number;
    /** A process this controller's owner ran before the page loaded, taken over if it is still there. */
    readonly resumePtyId?: number;
    /** The process this controller owns, whether spawned or taken over. */
    readonly onProcess?: (id: number) => void;
    readonly cwd?: string;
    readonly startup?: string;
    readonly directCommand?: PtyDirectCommand;
    readonly context?: Context;
    readonly cols?: number;
    readonly rows?: number;
    /** Pasted once the program starts and never submitted, so it waits at the prompt to be edited or sent. */
    readonly initialDraft?: string;
    readonly onInitialInputDelivered?: () => void;
    readonly onError?: (event: PtyControllerErrorEvent) => void;
    readonly maxPendingChunks?: number;
    readonly maxPendingBytes?: number;
    readonly maxSnapshotBytes?: number;
    readonly maxListenerErrors?: number;
    readonly maxStateListeners?: number;
    readonly maxAttachments?: number;
}

export class PtyControllerDisposedError extends Error {
    constructor() {
        super("PTY controller is disposing or disposed");
        this.name = "PtyControllerDisposedError";
    }
}

export class PtyControllerExitedError extends Error {
    constructor() {
        super("PTY process has exited; adopt a replacement or create a new controller to restart it");
        this.name = "PtyControllerExitedError";
    }
}

export class PtySubscriptionOverflowError extends Error {
    constructor() {
        super("PTY renderer fell behind before applying its atomic snapshot; reattach to resynchronize");
        this.name = "PtySubscriptionOverflowError";
    }
}

export class PtyControllerReplacedError extends Error {
    constructor() {
        super("PTY controller moved to a replacement externally owned process");
        this.name = "PtyControllerReplacedError";
    }
}

type StateListener = (snapshot: PtyControllerSnapshot) => void;

type AttachmentState<ChannelTransport> = {
    readonly localId: number;
    readonly ptyId: number;
    readonly ptyGeneration: number;
    readonly listener: (chunk: PtyOutputChunk) => void;
    readonly binding: PtyChannelBinding<ChannelTransport>;
    nativeAttach: Promise<PtyAttachResult>;
    subId: number | null;
    queue: PtyOutputChunk[];
    queuedBytes: number;
    active: boolean;
    delivering: boolean;
    detached: boolean;
    overflowed: boolean;
    listenerErrors: number;
    detachPromise: Promise<void> | null;
};

type PendingResize = {
    generation: number;
    cols: number;
    rows: number;
    readonly promise: Promise<void>;
    readonly resolve: () => void;
    readonly reject: (error: unknown) => void;
};

const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;
const DEFAULT_MAX_PENDING_CHUNKS = 256;
const DEFAULT_MAX_PENDING_BYTES = 1024 * 1024;
const DEFAULT_MAX_SNAPSHOT_BYTES = 8 * 1024 * 1024;
const DEFAULT_MAX_LISTENER_ERRORS = 8;
const DEFAULT_MAX_STATE_LISTENERS = 32;
const DEFAULT_MAX_ATTACHMENTS = 16;
const MAX_SUBSCRIPTION_ID = 0xffff_ffff;
const SHELL_PHASES = new Set<PtyShellPhase>(["unknown", "prompt", "input", "running", "finished"]);

function requirePositiveInteger(name: string, value: number, max = Number.MAX_SAFE_INTEGER): number {
    if (!Number.isSafeInteger(value) || value <= 0 || value > max) {
        throw new RangeError(`${name} must be a positive integer no greater than ${max}`);
    }
    return value;
}

function isSubscriptionId(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0 && (value as number) <= MAX_SUBSCRIPTION_ID;
}

function isPtyId(value: unknown): value is number {
    return Number.isSafeInteger(value) && (value as number) >= 0;
}

function requirePtyId(name: string, value: unknown): number {
    if (!isPtyId(value)) throw new RangeError(`${name} must be a non-negative safe integer`);
    return value;
}

function callAsPromise<T>(operation: () => Promise<T>): Promise<T> {
    try {
        return operation();
    } catch (error) {
        return Promise.reject(error);
    }
}

/**
 * Public controller methods are deliberately safe to fire-and-forget. Attaching
 * a rejection observer here prevents an ignored operation from surfacing as a
 * global unhandled rejection while preserving the original promise (and its
 * rejection) for callers that await it.
 */
function containRejection<T>(promise: Promise<T>): Promise<T> {
    void promise.catch(() => {});
    return promise;
}

/** Raw IPC bytes arrive as an ArrayBuffer; the view shares the same memory. */
function asOutputChunk(value: unknown): PtyOutputChunk | null {
    if (Array.isArray(value) || value instanceof Uint8Array) return value;
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    return null;
}

export function parsePtyShellMetadataSnapshot(value: unknown): PtyShellMetadataSnapshot | null {
    if (!isRecord(value)) return null;
    const { revision, cwd, phase, lastExitCode } = value;
    if (typeof revision !== "number" || typeof phase !== "string" || !SHELL_PHASES.has(phase as PtyShellPhase)) return null;
    if (cwd !== null && typeof cwd !== "string") return null;
    if (lastExitCode !== null && typeof lastExitCode !== "number") return null;
    return { revision, cwd, phase: phase as PtyShellPhase, lastExitCode };
}

function validateAttachResult(result: PtyAttachResult, maxSnapshotBytes: number): PtyShellMetadataSnapshot | null {
    if (!isSubscriptionId(result.subId)) throw new TypeError("PTY attach returned an invalid subscription ID");
    if (!(result.frame instanceof Uint8Array) || result.frame.length > maxSnapshotBytes) {
        throw new TypeError("PTY attach returned an invalid or oversized frame");
    }
    if (result.shell === undefined || result.shell === null) return null;
    const shell = parsePtyShellMetadataSnapshot(result.shell);
    if (!shell) throw new TypeError("PTY attach returned invalid shell metadata");
    return shell;
}

/** Only native errors that explicitly identify a gone PTY end process ownership. */
function isTerminalAttachFailure(error: unknown): boolean {
    if (error instanceof PtyControllerExitedError) return true;
    const category = errorCategory(error);
    if (category === "not-found" || category === "pty-not-found" || category === "exited" || category === "pty-exited") return true;
    // Current native `pty_attach` reports its exact lookup miss as AppError::BadArg.
    // Match the complete typed wire pair so unrelated bad arguments stay retryable.
    return category === "bad-arg" && errorText(error) === "invalid argument: pty not found";
}

/**
 * Runtime-only PTY lifecycle. Controller-owned spawns are killed on disposal;
 * externally owned processes only lose their renderer subscriptions. Spawn and
 * renderer-attach failures may be retried, while external replacements use a
 * monotonic generation so stale attachment completions cannot affect the new PTY.
 */
export class PtyLifecycleController<ChannelTransport, Context = unknown> {
    private readonly api: PtyApi<ChannelTransport, Context>;
    private readonly channels: PtyChannelAdapter<ChannelTransport>;
    private readonly spawnRequest: PtySpawnRequest<Context>;
    private readonly processOwnership: PtyProcessOwnership;
    private readonly onInitialInputDelivered?: () => void;
    private readonly onError?: (event: PtyControllerErrorEvent) => void;
    private readonly maxPendingChunks: number;
    private readonly maxPendingBytes: number;
    private readonly maxSnapshotBytes: number;
    private readonly maxListenerErrors: number;
    private readonly maxStateListeners: number;
    private readonly maxAttachments: number;
    private readonly attachments = new Map<number, AttachmentState<ChannelTransport>>();
    private readonly stateListeners = new Set<StateListener>();
    private status: PtyControllerStatus = "idle";
    private failureOperation: PtyOperation | null = null;
    private initialInputStatus: PtyInitialInputStatus;
    private initialDraft: string | null;
    private initialInputAttempted = false;
    private externalPtyId: number | null;
    private resumePtyId: number | null;
    private readonly onProcess?: (id: number) => void;
    private ptyId: number | null = null;
    private ptyGeneration = 0;
    private startPromise: Promise<number> | null = null;
    private disposePromise: Promise<void> | null = null;
    private killPromise: Promise<void> | null = null;
    private disposeRequested = false;
    private spawnAttempts = 0;
    private revision = 0;
    private attachmentSequence = 0;
    private resizeSequence = 0;
    private pendingResize: PendingResize | null = null;
    private resizeDrainPromise: Promise<void> | null = null;
    private cols: number;
    private rows: number;

    constructor(options: PtyLifecycleControllerOptions<ChannelTransport, Context>) {
        this.api = options.api;
        this.channels = options.channels;
        this.externalPtyId = options.existingPtyId === undefined ? null : requirePtyId("existingPtyId", options.existingPtyId);
        this.processOwnership = this.externalPtyId === null ? "controller" : "external";
        this.resumePtyId = options.resumePtyId === undefined || this.externalPtyId !== null ? null : requirePtyId("resumePtyId", options.resumePtyId);
        this.onProcess = options.onProcess;
        if (this.externalPtyId !== null) this.ptyGeneration = 1;
        this.cols = requirePositiveInteger("cols", options.cols ?? DEFAULT_COLS, 65_535);
        this.rows = requirePositiveInteger("rows", options.rows ?? DEFAULT_ROWS, 65_535);
        this.spawnRequest = Object.freeze({
            cols: this.cols,
            rows: this.rows,
            cwd: options.cwd ?? null,
            startup: options.directCommand ? null : (options.startup ?? null),
            directCommand: options.directCommand ?? null,
            context: options.context ?? null,
        });
        this.maxPendingChunks = requirePositiveInteger("maxPendingChunks", options.maxPendingChunks ?? DEFAULT_MAX_PENDING_CHUNKS);
        this.maxPendingBytes = requirePositiveInteger("maxPendingBytes", options.maxPendingBytes ?? DEFAULT_MAX_PENDING_BYTES);
        this.maxSnapshotBytes = requirePositiveInteger("maxSnapshotBytes", options.maxSnapshotBytes ?? DEFAULT_MAX_SNAPSHOT_BYTES);
        this.maxListenerErrors = requirePositiveInteger("maxListenerErrors", options.maxListenerErrors ?? DEFAULT_MAX_LISTENER_ERRORS);
        this.maxStateListeners = requirePositiveInteger("maxStateListeners", options.maxStateListeners ?? DEFAULT_MAX_STATE_LISTENERS);
        this.maxAttachments = requirePositiveInteger("maxAttachments", options.maxAttachments ?? DEFAULT_MAX_ATTACHMENTS);
        this.onInitialInputDelivered = options.onInitialInputDelivered;
        this.onError = options.onError;
        this.initialDraft = options.initialDraft?.trim() ? options.initialDraft : null;
        this.initialInputStatus = this.initialDraft ? "pending" : "none";
    }

    getSnapshot(): PtyControllerSnapshot {
        return Object.freeze({
            revision: this.revision,
            status: this.status,
            spawnAttempts: this.spawnAttempts,
            attachmentCount: this.attachments.size,
            initialInput: this.initialInputStatus,
            cols: this.cols,
            rows: this.rows,
            failureOperation: this.failureOperation,
            processOwnership: this.processOwnership,
        });
    }

    subscribe(listener: StateListener): () => void {
        if (this.stateListeners.size >= this.maxStateListeners) throw new RangeError("PTY controller state listener limit reached");
        this.stateListeners.add(listener);
        if (!this.deliverState(listener)) this.stateListeners.delete(listener);
        return () => this.stateListeners.delete(listener);
    }

    start(): Promise<number> {
        if (this.status === "disposing" || this.status === "disposed" || this.disposeRequested) {
            return containRejection(Promise.reject(new PtyControllerDisposedError()));
        }
        if (this.status === "exited") return containRejection(Promise.reject(new PtyControllerExitedError()));
        if (this.ptyId !== null && this.status === "running") return Promise.resolve(this.ptyId);
        if (this.startPromise) return this.startPromise;
        if (this.processOwnership === "external") return this.startExternalPty();

        this.status = "starting";
        this.failureOperation = null;
        this.spawnAttempts += 1;
        this.emitState();
        let resumed = false;
        const spawned = this.resumeOrSpawn().then((outcome) => {
            resumed = outcome.resumed;
            return outcome.id;
        });
        const attempt = spawned
            .then(async (id) => {
                if (!isPtyId(id)) throw new TypeError("PTY spawn returned an invalid runtime ID");
                this.ptyGeneration += 1;
                this.ptyId = id;
                if (this.disposeRequested) {
                    try {
                        await this.killOnce(id);
                    } catch {
                        // killOnce already reports; disposal remains best-effort.
                    }
                    throw new PtyControllerDisposedError();
                }
                this.status = "running";
                this.failureOperation = null;
                if (resumed) this.skipInitialInput();
                this.emitState();
                this.notifyProcess(id);
                void this.deliverInitialInput(id);
                return id;
            })
            .catch((error: unknown) => {
                if (this.disposeRequested) throw new PtyControllerDisposedError();
                this.reportError("spawn", error);
                // Spawn failure is explicitly retryable: startPromise is cleared
                // below, and the next start() makes a fresh native attempt.
                this.status = "failed";
                this.failureOperation = "spawn";
                this.emitState();
                throw error;
            });
        const tracked: Promise<number> = containRejection(
            attempt.finally(() => {
                if (this.startPromise === tracked) this.startPromise = null;
            }),
        );
        this.startPromise = tracked;
        return tracked;
    }

    /** A taken-over process already had its first input, so it never gets it twice. */
    private async resumeOrSpawn(): Promise<{ id: number; resumed: boolean }> {
        const candidate = this.resumePtyId;
        this.resumePtyId = null;
        if (candidate !== null && this.api.resume) {
            const available = await callAsPromise(() => this.api.resume!(candidate)).catch(() => false);
            if (available) return { id: candidate, resumed: true };
        }
        return { id: await callAsPromise(() => this.api.spawn(this.spawnRequest)), resumed: false };
    }

    private skipInitialInput(): void {
        this.initialDraft = null;
        this.initialInputAttempted = true;
        if (this.initialInputStatus === "pending") this.initialInputStatus = "none";
    }

    private notifyProcess(id: number): void {
        try {
            this.onProcess?.(id);
        } catch (error) {
            this.reportError("spawn", error);
        }
    }

    private startExternalPty(): Promise<number> {
        const id = this.externalPtyId;
        if (id === null) return containRejection(Promise.reject(new TypeError("external PTY target is missing")));
        const generation = this.ptyGeneration;
        this.status = "starting";
        this.failureOperation = null;
        this.emitState();

        const attempt = Promise.resolve().then(() => {
            if (this.disposeRequested) throw new PtyControllerDisposedError();
            if (generation !== this.ptyGeneration || id !== this.externalPtyId) throw new PtyControllerReplacedError();
            this.ptyId = id;
            this.status = "running";
            this.failureOperation = null;
            this.emitState();
            void this.deliverInitialInput(id);
            return id;
        });
        const tracked: Promise<number> = containRejection(
            attempt.finally(() => {
                if (this.startPromise === tracked) this.startPromise = null;
            }),
        );
        this.startPromise = tracked;
        return tracked;
    }

    /**
     * Point an external controller at a replacement PTY without taking process
     * ownership. Old renderer subscriptions detach from their exact PTY ID.
     */
    adoptExistingPty(idInput: number): Promise<number> {
        let id: number;
        try {
            id = requirePtyId("existingPtyId", idInput);
        } catch (error) {
            return containRejection(Promise.reject(error));
        }
        if (this.processOwnership !== "external") {
            return containRejection(Promise.reject(new TypeError("a controller-owned PTY cannot adopt an external process")));
        }
        if (this.disposeRequested || this.status === "disposing" || this.status === "disposed") {
            return containRejection(Promise.reject(new PtyControllerDisposedError()));
        }
        if (this.externalPtyId === id && (this.status === "starting" || (this.status === "running" && this.ptyId === id))) {
            return this.start();
        }
        if (this.ptyGeneration >= Number.MAX_SAFE_INTEGER) {
            return containRejection(Promise.reject(new RangeError("PTY ownership generation space is exhausted")));
        }

        const lifecycleStarted = this.status !== "idle";
        this.ptyGeneration += 1;
        this.externalPtyId = id;
        this.ptyId = null;
        this.startPromise = null;
        this.resizeSequence += 1;
        this.rejectPendingResizes(new PtyControllerReplacedError());
        if (lifecycleStarted && (this.initialInputStatus === "pending" || this.initialInputStatus === "delivering")) {
            this.initialInputStatus = "cancelled";
            this.cancelInitialInput();
        }
        for (const attachment of Array.from(this.attachments.values())) void this.detachAttachment(attachment).catch(swallow("detach a terminal"));
        this.status = "idle";
        this.failureOperation = null;
        this.emitState();
        return this.start();
    }

    write(data: string): Promise<void> {
        return containRejection(this.performWrite(data));
    }

    private async performWrite(data: string): Promise<void> {
        const id = await this.readyPtyId();
        this.assertRunning(id);
        try {
            await this.api.write(id, data);
        } catch (error) {
            if (!this.disposeRequested && this.status === "running" && this.ptyId === id) {
                this.failureOperation = "write";
                this.reportError("write", error);
                this.emitState();
            }
            throw error;
        }
    }

    input(input: TerminalInput): Promise<void> {
        return containRejection(this.performInput(input));
    }

    private async performInput(input: TerminalInput): Promise<void> {
        const id = await this.readyPtyId();
        this.assertRunning(id);
        try {
            await this.api.input(id, input);
        } catch (error) {
            if (!this.disposeRequested && this.status === "running" && this.ptyId === id) {
                this.failureOperation = "input";
                this.reportError("input", error);
                this.emitState();
            }
            throw error;
        }
    }

    resize(cols: number, rows: number): Promise<void> {
        try {
            requirePositiveInteger("cols", cols, 65_535);
            requirePositiveInteger("rows", rows, 65_535);
        } catch (error) {
            return containRejection(Promise.reject(error));
        }

        const generation = ++this.resizeSequence;
        if (this.pendingResize) {
            this.pendingResize.generation = generation;
            this.pendingResize.cols = cols;
            this.pendingResize.rows = rows;
            return this.pendingResize.promise;
        }

        let resolve!: () => void;
        let reject!: (error: unknown) => void;
        const promise = containRejection(
            new Promise<void>((resolvePromise, rejectPromise) => {
                resolve = resolvePromise;
                reject = rejectPromise;
            }),
        );
        this.pendingResize = { generation, cols, rows, promise, resolve, reject };
        this.ensureResizeDrain();
        return promise;
    }

    attach(listener: (chunk: PtyOutputChunk) => void): Promise<PtyAttachment> {
        return containRejection(this.performAttach(listener));
    }

    private async performAttach(listener: (chunk: PtyOutputChunk) => void): Promise<PtyAttachment> {
        const id = await this.readyPtyId();
        this.assertRunning(id);
        if (this.attachments.size >= this.maxAttachments) throw new RangeError("PTY controller attachment limit reached");

        let state: AttachmentState<ChannelTransport> | null = null;
        let binding: PtyChannelBinding<ChannelTransport>;
        try {
            binding = this.channels.create((chunk) => {
                if (state) this.receiveOutput(state, chunk);
            });
        } catch (error) {
            this.reportError("attach", error);
            throw error;
        }
        let resolveNative!: (result: PtyAttachResult) => void;
        let rejectNative!: (error: unknown) => void;
        const nativeAttach = new Promise<PtyAttachResult>((resolve, reject) => {
            resolveNative = resolve;
            rejectNative = reject;
        });
        state = {
            localId: ++this.attachmentSequence,
            ptyId: id,
            ptyGeneration: this.ptyGeneration,
            listener,
            binding,
            nativeAttach,
            subId: null,
            queue: [],
            queuedBytes: 0,
            active: false,
            delivering: false,
            detached: false,
            overflowed: false,
            listenerErrors: 0,
            detachPromise: null,
        };
        this.attachments.set(state.localId, state);
        this.emitState();
        callAsPromise(() => this.api.attach(id, binding.transport)).then(resolveNative, rejectNative);

        let attached: PtyAttachResult;
        let shell: PtyShellMetadataSnapshot | null;
        try {
            attached = await nativeAttach;
            if (isSubscriptionId(attached.subId)) state.subId = attached.subId;
            shell = validateAttachResult(attached, this.maxSnapshotBytes);
        } catch (error) {
            const stale = this.isStaleAttachment(state);
            await this.abandonAttachment(state);
            if (this.disposeRequested) throw new PtyControllerDisposedError();
            if (stale) throw new PtyControllerReplacedError();
            if (isTerminalAttachFailure(error)) {
                this.markExited("attach");
            } else if (this.status === "running") {
                this.failureOperation = "attach";
                this.emitState();
            }
            this.reportError("attach", error);
            throw error;
        }

        const stale = this.isStaleAttachment(state);
        if (state.detached || this.disposeRequested || stale) {
            await this.detachAttachment(state).catch(swallow("detach a terminal"));
            if (state.overflowed) throw new PtySubscriptionOverflowError();
            if (stale && !this.disposeRequested) throw new PtyControllerReplacedError();
            throw new PtyControllerDisposedError();
        }

        if (this.failureOperation === "attach") {
            this.failureOperation = null;
            this.emitState();
        }

        const attachment: PtyAttachment = Object.freeze({
            frame: attached.frame,
            shell,
            activate: () => this.activateAttachment(state),
            ack: (frame: number) => this.ackAttachment(state, frame),
            detach: () => this.detachAttachment(state),
            toJSON: (): never => {
                throw new TypeError("PTY attachments contain runtime output and cannot be serialized");
            },
        });
        return attachment;
    }

    dispose(): Promise<void> {
        if (this.disposePromise) return this.disposePromise;
        this.disposeRequested = true;
        if (this.initialInputStatus === "pending" || this.initialInputStatus === "delivering") {
            this.initialInputStatus = "cancelled";
        }
        this.cancelInitialInput();
        if (this.status !== "disposed") {
            this.status = "disposing";
            this.failureOperation = null;
            this.emitState();
        }
        this.rejectPendingResizes(new PtyControllerDisposedError());

        const starting = this.startPromise;
        const detaches = Array.from(this.attachments.values(), (attachment) => this.detachAttachment(attachment));
        const runningId = this.ptyId;
        const immediateKill = this.processOwnership === "controller" && runningId !== null ? this.killOnce(runningId) : null;
        // Attach rejection handlers immediately; a slow detach must not leave a
        // concurrently failing start/kill promise temporarily unhandled.
        const settledDetaches = Promise.allSettled(detaches);
        const settledStart = starting ? Promise.allSettled([starting]) : null;
        const settledKill = immediateKill ? Promise.allSettled([immediateKill]) : null;
        this.disposePromise = containRejection(
            (async () => {
                await settledDetaches;
                if (settledStart) await settledStart;
                if (settledKill) await settledKill;
                else if (this.processOwnership === "controller" && this.ptyId !== null) {
                    await Promise.allSettled([this.killOnce(this.ptyId)]);
                }
                this.ptyId = null;
                this.initialDraft = null;
                this.status = "disposed";
                this.failureOperation = null;
                this.emitState();
                this.stateListeners.clear();
            })(),
        );
        return this.disposePromise;
    }

    toJSON(): never {
        throw new TypeError("PTY controllers hold runtime process handles and cannot be serialized");
    }

    private async readyPtyId(): Promise<number> {
        if (this.status === "exited") throw new PtyControllerExitedError();
        if (this.status === "disposing" || this.status === "disposed" || this.disposeRequested) throw new PtyControllerDisposedError();
        return this.ptyId ?? this.start();
    }

    private assertRunning(id: number): void {
        if (this.status === "exited") throw new PtyControllerExitedError();
        if (!this.disposeRequested && this.processOwnership === "external" && this.ptyId !== id) {
            throw new PtyControllerReplacedError();
        }
        if (this.disposeRequested || this.status !== "running" || this.ptyId !== id) throw new PtyControllerDisposedError();
    }

    private ensureResizeDrain(): void {
        if (this.resizeDrainPromise) return;
        // Start on the next microtask so same-turn ResizeObserver bursts reduce
        // to one native call before any IPC is dispatched.
        const drain = containRejection(Promise.resolve().then(() => this.drainResizeQueue()));
        this.resizeDrainPromise = drain;
        void drain
            .finally(() => {
                if (this.resizeDrainPromise === drain) this.resizeDrainPromise = null;
                // A request can arrive after the drain observes an empty queue but
                // before this completion callback runs.
                if (this.pendingResize) this.ensureResizeDrain();
            })
            .catch(swallow("drain terminal resizes"));
    }

    private async drainResizeQueue(): Promise<void> {
        while (this.pendingResize) {
            const request = this.pendingResize;
            this.pendingResize = null;

            let id: number;
            try {
                id = await this.readyPtyId();
                this.assertRunning(id);
            } catch (error) {
                request.reject(error);
                this.rejectPendingResizes(error);
                return;
            }

            try {
                // Native resize calls are strictly serialized. New requests
                // replace one pending value and run only after this call ends.
                await callAsPromise(() => this.api.resize(id, request.cols, request.rows));
            } catch (error) {
                // A failed obsolete resize is observable to its direct caller,
                // but must not overwrite state or telemetry for a newer size.
                if (request.generation === this.resizeSequence && !this.disposeRequested && this.status === "running" && this.ptyId === id) {
                    this.failureOperation = "resize";
                    this.emitState();
                    if (request.generation === this.resizeSequence && !this.disposeRequested && this.status === "running" && this.ptyId === id) {
                        this.reportError("resize", error);
                    }
                }
                request.reject(error);
                continue;
            }

            try {
                this.assertRunning(id);
            } catch (error) {
                request.reject(error);
                this.rejectPendingResizes(error);
                return;
            }

            if (request.generation === this.resizeSequence) {
                this.cols = request.cols;
                this.rows = request.rows;
                if (this.failureOperation === "resize") this.failureOperation = null;
                this.emitState();
            }
            request.resolve();
        }
    }

    private rejectPendingResizes(error: unknown): void {
        const pending = this.pendingResize;
        this.pendingResize = null;
        pending?.reject(error);
    }

    private async deliverInitialInput(id: number): Promise<void> {
        const draft = this.initialDraft;
        if (this.initialInputAttempted || !draft) return;
        this.initialDraft = null;
        if (this.disposeRequested || this.status !== "running" || this.ptyId !== id) return;
        this.initialInputAttempted = true;
        this.initialInputStatus = "delivering";
        this.emitState();
        try {
            await this.api.typeWhenReady(id, draft);
            if (this.disposeRequested || this.ptyId !== id) {
                this.initialInputStatus = "cancelled";
                this.emitState();
                return;
            }
            if (this.getSnapshot().status === "exited") {
                this.initialInputStatus = "failed";
                this.emitState();
                return;
            }
            this.initialInputStatus = "delivered";
            this.failureOperation = null;
            if (!this.disposeRequested) {
                try {
                    this.onInitialInputDelivered?.();
                } catch (error) {
                    this.reportError("initial-input-callback", error);
                }
            }
        } catch (error) {
            if (this.disposeRequested || this.ptyId !== id) {
                this.initialInputStatus = "cancelled";
                this.emitState();
                return;
            }
            // At-most-once policy: a rejected bridge response is ambiguous, so
            // automatic retry could submit the user's first task twice.
            this.initialInputStatus = "failed";
            this.failureOperation = "initial-input";
            this.reportError("initial-input", error);
        }
        this.emitState();
    }

    private cancelInitialInput(): void {
        this.initialDraft = null;
    }

    private activateAttachment(state: AttachmentState<ChannelTransport>): void {
        if (state.overflowed) throw new PtySubscriptionOverflowError();
        if (state.detached) return;
        state.active = true;
        this.drainOutput(state);
    }

    private ackAttachment(state: AttachmentState<ChannelTransport>, frame: number): void {
        const subId = state.subId;
        if (state.detached || subId === null || !Number.isSafeInteger(frame) || frame <= 0) return;
        void callAsPromise(() => this.api.ack(state.ptyId, subId, frame)).catch((error: unknown) => {
            this.reportError("ack", error);
        });
    }

    private receiveOutput(state: AttachmentState<ChannelTransport>, received: PtyOutputChunk): void {
        if (state.detached) return;
        const chunk = asOutputChunk(received);
        if (!chunk) {
            this.reportError("attach", new TypeError("PTY channel emitted an invalid output chunk"));
            void this.detachAttachment(state).catch(swallow("detach a terminal"));
            return;
        }
        if (chunk.length > this.maxPendingBytes) {
            state.overflowed = true;
            this.reportError("attach", new PtySubscriptionOverflowError());
            void this.detachAttachment(state).catch(swallow("detach a terminal"));
            return;
        }
        if (chunk.length === 0) this.markExited("exit");
        if (state.active && !state.delivering && state.queue.length === 0) {
            this.deliverOutput(state, chunk);
            this.drainOutput(state);
            return;
        }
        if (state.queue.length >= this.maxPendingChunks || state.queuedBytes + chunk.length > this.maxPendingBytes) {
            state.overflowed = true;
            state.queue = [];
            state.queuedBytes = 0;
            this.reportError("attach", new PtySubscriptionOverflowError());
            void this.detachAttachment(state).catch(swallow("detach a terminal"));
            return;
        }
        state.queue.push(chunk);
        state.queuedBytes += chunk.length;
    }

    private drainOutput(state: AttachmentState<ChannelTransport>): void {
        while (state.active && !state.detached && !state.delivering && state.queue.length > 0) {
            const chunk = state.queue.shift();
            if (!chunk) continue;
            state.queuedBytes -= chunk.length;
            this.deliverOutput(state, chunk);
        }
    }

    private deliverOutput(state: AttachmentState<ChannelTransport>, chunk: PtyOutputChunk): void {
        if (state.detached) return;
        state.delivering = true;
        try {
            state.listener(chunk);
        } catch (error) {
            state.listenerErrors += 1;
            this.reportError("output-listener", error);
            if (state.listenerErrors >= this.maxListenerErrors) void this.detachAttachment(state).catch(swallow("detach a terminal"));
        } finally {
            state.delivering = false;
        }
    }

    private isStaleAttachment(state: AttachmentState<ChannelTransport>): boolean {
        return state.ptyGeneration !== this.ptyGeneration || state.ptyId !== this.ptyId;
    }

    private detachAttachment(state: AttachmentState<ChannelTransport>): Promise<void> {
        if (state.detachPromise) return state.detachPromise;
        state.detached = true;
        state.active = false;
        state.queue = [];
        state.queuedBytes = 0;
        this.attachments.delete(state.localId);
        try {
            state.binding.close();
        } catch (error) {
            this.reportError("channel-close", error);
        }
        this.emitState();
        state.detachPromise = containRejection(
            state.nativeAttach.then(
                async (attached) => {
                    if (!isSubscriptionId(attached.subId)) return;
                    state.subId = attached.subId;
                    try {
                        await this.api.detach(state.ptyId, attached.subId);
                    } catch (error) {
                        this.reportError("detach", error);
                        throw error;
                    }
                },
                () => {},
            ),
        );
        return state.detachPromise;
    }

    private async abandonAttachment(state: AttachmentState<ChannelTransport>): Promise<void> {
        if (state.detached) {
            if (state.detachPromise) await state.detachPromise.catch(() => {});
            return;
        }
        state.detached = true;
        state.active = false;
        state.queue = [];
        state.queuedBytes = 0;
        this.attachments.delete(state.localId);
        try {
            state.binding.close();
        } catch (error) {
            this.reportError("channel-close", error);
        }
        if (state.subId !== null) {
            try {
                await this.api.detach(state.ptyId, state.subId);
            } catch (error) {
                this.reportError("detach", error);
            }
        }
        this.emitState();
    }

    private markExited(operation: "attach" | "exit"): void {
        if (this.status !== "running") return;
        this.cancelInitialInput();
        if (this.initialInputStatus === "pending" || this.initialInputStatus === "delivering") {
            this.initialInputStatus = "failed";
        }
        this.status = "exited";
        this.failureOperation = operation;
        this.emitState();
    }

    private killOnce(id: number): Promise<void> {
        if (this.killPromise) return this.killPromise;
        this.killPromise = containRejection(
            callAsPromise(() => this.api.kill(id))
                .catch((error) => {
                    this.failureOperation = "kill";
                    this.reportError("kill", error);
                    throw error;
                })
                .finally(() => {
                    if (this.ptyId === id) this.ptyId = null;
                }),
        );
        return this.killPromise;
    }

    private emitState(): void {
        this.revision += 1;
        const snapshot = this.getSnapshot();
        for (const listener of Array.from(this.stateListeners)) {
            if (!this.deliverState(listener, snapshot)) this.stateListeners.delete(listener);
        }
    }

    private deliverState(listener: StateListener, snapshot = this.getSnapshot()): boolean {
        try {
            listener(snapshot);
            return true;
        } catch (error) {
            this.reportError("state-listener", error);
            return false;
        }
    }

    private reportError(operation: PtyOperation, error: unknown): void {
        try {
            this.onError?.({ operation, error });
        } catch {
            // Observability must never break PTY ownership or channel delivery.
        }
    }
}
