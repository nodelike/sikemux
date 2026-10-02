import { beforeEach, describe, expect, it, vi } from "vitest";
import {
    MemoryIpcTransport,
    createIpcTransport,
    getIpcTransport,
    installIpcTransportForTests,
    productionIpcTransport,
    resetIpcTransportForTests,
    type IpcEventListener,
    type IpcTransportBindings,
} from "./transport";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

function deferred<T>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

function bindings(overrides: Partial<IpcTransportBindings> = {}): IpcTransportBindings {
    return {
        invoke: async () => undefined as never,
        subscribe: async () => () => {},
        ...overrides,
    };
}

beforeEach(() => {
    resetIpcTransportForTests();
    mocks.invoke.mockReset();
    mocks.listen.mockReset();
});

describe("production IPC transport", () => {
    it("is the default and preserves opaque Channel-compatible args, native options, and call arity", async () => {
        let enumerations = 0;
        const args = new Proxy(
            { opaqueChannel: Object.freeze({ id: 7 }) },
            {
                ownKeys: () => {
                    enumerations += 1;
                    throw new Error("transport enumerated opaque args");
                },
                getOwnPropertyDescriptor: () => {
                    enumerations += 1;
                    throw new Error("transport inspected opaque args");
                },
            },
        );
        const native = Object.freeze({ headers: Object.freeze({ "x-test": "opaque" }) });
        const result = Object.freeze({ marker: "result" });
        mocks.invoke.mockImplementation((_command, receivedArgs, receivedNative) => {
            expect(receivedArgs).toBe(args);
            expect(receivedNative).toBe(native);
            return Promise.resolve(result);
        });

        expect(getIpcTransport()).toBe(productionIpcTransport);
        await expect(getIpcTransport().invoke("pty_attach", args, { native })).resolves.toBe(result);
        expect(enumerations).toBe(0);
        expect(mocks.invoke.mock.calls[0]).toHaveLength(3);

        mocks.invoke.mockClear();
        mocks.invoke.mockResolvedValue(undefined);
        await getIpcTransport().invoke("integration_health");
        expect(mocks.invoke.mock.calls[0]).toEqual(["integration_health"]);
    });

    it("propagates cancellation and observes late invoke settlement", async () => {
        const pending = deferred<unknown>();
        const invoke = vi.fn(() => pending.promise);
        const transport = createIpcTransport(
            bindings({
                invoke: <Result>() => invoke() as Promise<Result>,
            }),
        );
        const controller = new AbortController();
        const reason = new Error("cancel invoke");
        const invocation = transport.invoke("slow_command", Object.freeze({ id: 1 }), { signal: controller.signal });

        controller.abort(reason);
        await expect(invocation).rejects.toBe(reason);
        pending.reject(new Error("late native rejection"));
        await Promise.resolve();

        const preAborted = new AbortController();
        preAborted.abort(reason);
        await expect(transport.invoke("never_called", undefined, { signal: preAborted.signal })).rejects.toBe(reason);
        expect(invoke).toHaveBeenCalledOnce();
    });

    it("cleans up a late subscription after abort and contains unsubscribe rejection", async () => {
        const pending = deferred<() => Promise<void>>();
        let nativeListener: IpcEventListener<number> | null = null;
        const subscribe = vi.fn((_event, listener) => {
            nativeListener = listener as IpcEventListener<number>;
            return pending.promise;
        });
        const unsubscribeError = new Error("late cleanup failed");
        const rawUnsubscribe = vi.fn(async () => {
            throw unsubscribeError;
        });
        const unsubscribeErrors: unknown[] = [];
        const transport = createIpcTransport(bindings({ subscribe }), {
            onUnsubscribeError: (_event, error) => unsubscribeErrors.push(error),
        });
        const controller = new AbortController();
        const reason = new Error("cancel subscribe");
        const listener = vi.fn();
        const subscribing = transport.subscribe("git_changed", listener, { signal: controller.signal });

        controller.abort(reason);
        await expect(subscribing).rejects.toBe(reason);
        pending.resolve(rawUnsubscribe);
        await vi.waitFor(() => expect(rawUnsubscribe).toHaveBeenCalledOnce());
        await vi.waitFor(() => expect(unsubscribeErrors).toEqual([unsubscribeError]));

        expect(() => nativeListener?.({ event: "git_changed", id: 1, payload: 7 })).not.toThrow();
        expect(listener).not.toHaveBeenCalled();
    });

    it("contains listener failures and abort-disposes an active typed subscription once", async () => {
        let nativeListener: IpcEventListener<{ readonly value: number }> | null = null;
        const rawUnsubscribe = vi.fn();
        const nativeOptions = Object.freeze({ target: "main" });
        mocks.listen.mockImplementation((_event, listener, receivedOptions) => {
            nativeListener = listener;
            expect(receivedOptions).toBe(nativeOptions);
            return Promise.resolve(rawUnsubscribe);
        });
        const listenerError = new Error("listener failed");
        const listenerErrors: unknown[] = [];
        const transport = createIpcTransport(
            {
                invoke: async () => undefined as never,
                subscribe: productionBindingSubscribe,
            },
            { onListenerError: (_event, error) => listenerErrors.push(error) },
        );
        const controller = new AbortController();
        const unlisten = await transport.subscribe<{ readonly value: number }>(
            "lsp_diagnostics",
            () => {
                throw listenerError;
            },
            { signal: controller.signal, native: nativeOptions },
        );

        const event = Object.freeze({ event: "lsp_diagnostics", id: 9, payload: Object.freeze({ value: 3 }) });
        expect(() => nativeListener?.(event)).not.toThrow();
        expect(listenerErrors).toEqual([listenerError]);

        controller.abort();
        unlisten();
        expect(rawUnsubscribe).toHaveBeenCalledOnce();
    });
});

async function productionBindingSubscribe<Payload>(event: string, listener: IpcEventListener<Payload>, nativeOptions?: unknown): Promise<() => void> {
    return mocks.listen(event, listener, nativeOptions) as Promise<() => void>;
}

describe("IPC test installation seam", () => {
    it("installs one isolated override and restores the production default explicitly", async () => {
        const memory = new MemoryIpcTransport();
        memory.register("ping", async () => "pong");
        const reset = installIpcTransportForTests(memory);

        expect(getIpcTransport()).toBe(memory);
        await expect(getIpcTransport().invoke("ping")).resolves.toBe("pong");
        expect(() => installIpcTransportForTests(new MemoryIpcTransport())).toThrow("already installed");

        reset();
        reset();
        expect(getIpcTransport()).toBe(productionIpcTransport);

        const staleReset = installIpcTransportForTests(memory);
        resetIpcTransportForTests();
        expect(getIpcTransport()).toBe(productionIpcTransport);

        const currentReset = installIpcTransportForTests(memory);
        staleReset();
        expect(getIpcTransport()).toBe(memory);
        currentReset();
    });
});
