import { invoke as tauriInvoke, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { listen as tauriListen, type Event as TauriEvent, type Options as TauriEventOptions } from "@tauri-apps/api/event";
import { swallow } from "../state/toast";

export type IpcInvokeArguments = unknown;
export type IpcNativeOptions = unknown;

export interface IpcInvokeOptions {
    readonly signal?: AbortSignal;
    /** Opaque adapter-specific options, passed through without inspection. */
    readonly native?: IpcNativeOptions;
}

export interface IpcSubscribeOptions {
    readonly signal?: AbortSignal;
    /** Opaque adapter-specific options, passed through without inspection. */
    readonly native?: IpcNativeOptions;
}

export interface IpcEvent<Payload> {
    readonly event: string;
    readonly id: number;
    readonly payload: Payload;
}

export type IpcEventListener<Payload> = (event: IpcEvent<Payload>) => void;
export type IpcUnsubscribe = () => void;

export interface IpcTransport {
    invoke<Result>(command: string, args?: IpcInvokeArguments, options?: IpcInvokeOptions): Promise<Result>;
    subscribe<Payload>(event: string, listener: IpcEventListener<Payload>, options?: IpcSubscribeOptions): Promise<IpcUnsubscribe>;
}

export interface IpcTransportBindings {
    invoke<Result>(command: string, args?: IpcInvokeArguments, nativeOptions?: IpcNativeOptions): Promise<Result>;
    subscribe<Payload>(event: string, listener: IpcEventListener<Payload>, nativeOptions?: IpcNativeOptions): Promise<() => void | PromiseLike<void>>;
}

function abortReason(signal: AbortSignal): unknown {
    return signal.reason === undefined ? new DOMException("The IPC operation was aborted", "AbortError") : signal.reason;
}

/** Race cancellation while retaining handlers on the late transport promise. */
function runAbortable<Result>(signal: AbortSignal | undefined, operation: () => Result | PromiseLike<Result>): Promise<Result> {
    if (signal?.aborted) return Promise.reject(abortReason(signal));
    let pending: Promise<Result>;
    try {
        pending = Promise.resolve(operation());
    } catch (error) {
        return Promise.reject(error);
    }
    if (!signal) return pending;
    return new Promise<Result>((resolve, reject) => {
        const onAbort = () => reject(abortReason(signal));
        signal.addEventListener("abort", onAbort, { once: true });
        pending.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
    });
}

function callInvokeBinding<Result>(
    binding: IpcTransportBindings["invoke"],
    command: string,
    args: IpcInvokeArguments | undefined,
    nativeOptions: IpcNativeOptions | undefined,
): Promise<Result> {
    // Tauri Channel arguments serialize through special hooks, so they must reach Tauri untouched and with the same arity.
    if (nativeOptions !== undefined) return binding<Result>(command, args, nativeOptions);
    if (args !== undefined) return binding<Result>(command, args);
    return binding<Result>(command);
}

export function createIpcTransport(bindings: IpcTransportBindings): IpcTransport {
    return {
        invoke: <Result>(command: string, args?: IpcInvokeArguments, options: IpcInvokeOptions = {}) =>
            runAbortable(options.signal, () => callInvokeBinding<Result>(bindings.invoke, command, args, options.native)),
        subscribe<Payload>(event: string, listener: IpcEventListener<Payload>, { signal, native }: IpcSubscribeOptions = {}) {
            if (signal?.aborted) return Promise.reject(abortReason(signal));
            return new Promise<IpcUnsubscribe>((resolve, reject) => {
                let live = true;
                let release = () => {};
                const unsubscribe = () => {
                    if (!live) return;
                    live = false;
                    signal?.removeEventListener("abort", onAbort);
                    release();
                };
                const onAbort = () => {
                    unsubscribe();
                    reject(abortReason(signal!));
                };
                signal?.addEventListener("abort", onAbort, { once: true });
                const forward: IpcEventListener<Payload> = (received) => {
                    if (live) listener(received);
                };
                const pending = native === undefined ? bindings.subscribe(event, forward) : bindings.subscribe(event, forward, native);
                pending.then(
                    (raw) => {
                        release = () => void Promise.resolve(raw()).catch(swallow(`stop listening for ${event}`));
                        if (live) resolve(unsubscribe);
                        else release();
                    },
                    (error: unknown) => {
                        signal?.removeEventListener("abort", onAbort);
                        reject(error);
                    },
                );
            });
        },
    };
}

export const productionIpcTransport: IpcTransport = createIpcTransport({
    invoke<Result>(command: string, args?: IpcInvokeArguments, nativeOptions?: IpcNativeOptions): Promise<Result> {
        const tauriArgs = args as InvokeArgs | undefined;
        if (nativeOptions !== undefined) return tauriInvoke<Result>(command, tauriArgs, nativeOptions as InvokeOptions);
        if (args !== undefined) return tauriInvoke<Result>(command, tauriArgs);
        return tauriInvoke<Result>(command);
    },
    subscribe<Payload>(event: string, listener: IpcEventListener<Payload>, nativeOptions?: IpcNativeOptions) {
        const tauriListener = listener as (event: TauriEvent<Payload>) => void;
        if (nativeOptions !== undefined) return tauriListen<Payload>(event, tauriListener, nativeOptions as TauriEventOptions);
        return tauriListen<Payload>(event, tauriListener);
    },
});

let testTransport: IpcTransport | null = null;

export function getIpcTransport(): IpcTransport {
    return testTransport ?? productionIpcTransport;
}

/** Install one override. Tests must reset before installing another. */
export function installIpcTransportForTests(transport: IpcTransport): () => void {
    if (testTransport) throw new Error("An IPC test transport is already installed");
    testTransport = transport;
    return () => {
        if (testTransport === transport) testTransport = null;
    };
}

export function resetIpcTransportForTests(): void {
    testTransport = null;
}

export interface MemoryInvokeContext {
    readonly command: string;
    readonly signal?: AbortSignal;
    readonly native?: IpcNativeOptions;
}

export type MemoryInvokeHandler = (args: IpcInvokeArguments | undefined, context: MemoryInvokeContext) => unknown;

/** An in-memory transport for frontend tests: registered commands answer invokes, and emit delivers events. */
export class MemoryIpcTransport implements IpcTransport {
    private readonly handlers = new Map<string, MemoryInvokeHandler>();
    private readonly coreRequests = new Map<string, MemoryInvokeHandler>();
    private readonly listeners = new Map<string, Set<IpcEventListener<unknown>>>();
    private eventSequence = 0;

    get commandHandlerCount(): number {
        return this.handlers.size;
    }

    get eventListenerCount(): number {
        let count = 0;
        for (const group of this.listeners.values()) count += group.size;
        return count;
    }

    register(command: string, handler: MemoryInvokeHandler): () => void {
        if (this.handlers.has(command)) throw new TypeError(`Memory IPC command already registered: ${command}`);
        this.handlers.set(command, handler);
        return () => {
            if (this.handlers.get(command) === handler) this.handlers.delete(command);
        };
    }

    /** Answers the core requests named `op` that the page sends through `core_request`. */
    registerCoreRequest(op: string, handler: MemoryInvokeHandler): void {
        if (this.coreRequests.size === 0) {
            this.register("core_request", (args, context) => {
                const request = (args as { request: { op: string } }).request;
                const answer = this.coreRequests.get(request.op);
                if (!answer) throw new Error(`No memory core request handler registered for ${request.op}`);
                return answer(request, context);
            });
        }
        if (this.coreRequests.has(op)) throw new TypeError(`Memory core request already registered: ${op}`);
        this.coreRequests.set(op, handler);
    }

    invoke<Result>(command: string, args?: IpcInvokeArguments, { signal, native }: IpcInvokeOptions = {}): Promise<Result> {
        const handler = this.handlers.get(command);
        if (!handler) return Promise.reject(new Error(`No memory IPC handler registered for ${command}`));
        const context: MemoryInvokeContext = { command, ...(signal && { signal }), ...(native !== undefined && { native }) };
        return runAbortable(signal, () => handler(args, context) as Result | PromiseLike<Result>);
    }

    subscribe<Payload>(event: string, listener: IpcEventListener<Payload>, { signal }: IpcSubscribeOptions = {}): Promise<IpcUnsubscribe> {
        if (signal?.aborted) return Promise.reject(abortReason(signal));
        const entry: IpcEventListener<unknown> = (received) => listener(received as IpcEvent<Payload>);
        const group = this.listeners.get(event) ?? new Set();
        group.add(entry);
        this.listeners.set(event, group);
        const unsubscribe = () => {
            signal?.removeEventListener("abort", unsubscribe);
            group.delete(entry);
            if (group.size === 0 && this.listeners.get(event) === group) this.listeners.delete(event);
        };
        signal?.addEventListener("abort", unsubscribe, { once: true });
        return Promise.resolve(unsubscribe);
    }

    /** Delivers `payload` to every listener for `event`, and says how many there were. */
    emit<Payload>(event: string, payload: Payload): number {
        this.eventSequence += 1;
        const received = { event, id: this.eventSequence, payload };
        const group = [...(this.listeners.get(event) ?? [])];
        for (const listener of group) listener(received);
        return group.length;
    }
}
