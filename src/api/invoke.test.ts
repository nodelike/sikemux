import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { uiActivity } from "../lib/activity";
import { performanceTelemetry } from "../lib/performance";
import {
    IPC_INVOKE_ACTIVE_GAUGE,
    IPC_INVOKE_CANCEL_COUNTER,
    IPC_INVOKE_ERROR_COUNTER,
    IPC_INVOKE_METRIC,
    IPC_INVOKE_SUCCESS_COUNTER,
    invokeCommand,
} from "./invoke";
import {
    MemoryIpcTransport,
    installIpcTransportForTests,
    resetIpcTransportForTests,
    type IpcInvokeArguments,
    type MemoryInvokeContext,
} from "./transport";

function deferred<T>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    performanceTelemetry.reset();
});

afterEach(() => {
    resetIpcTransportForTests();
});

describe("invokeCommand", () => {
    it("tracks deterministic concurrent success and categorized error outcomes", async () => {
        const success = deferred<{ readonly marker: "result" }>();
        const failure = deferred<never>();
        const successHandler = vi.fn((_args: IpcInvokeArguments | undefined, _context: MemoryInvokeContext) => success.promise);
        const failureHandler = vi.fn((_args: IpcInvokeArguments | undefined, _context: MemoryInvokeContext) => failure.promise);
        transport.register("files_call", successHandler);
        transport.register("state_save", failureHandler);

        const readArgs = { path: "/project/file" };
        const statusArgs = { repo: "/project" };
        const successful = invokeCommand<{ readonly marker: "result" }>("files_call", readArgs);
        const failed = invokeCommand<never>("state_save", statusArgs);

        expect(performanceTelemetry.snapshot().gauges[IPC_INVOKE_ACTIVE_GAUGE]).toBe(2);
        expect(successHandler.mock.calls[0]![0]).toBe(readArgs);
        expect(failureHandler.mock.calls[0]![0]).toBe(statusArgs);

        const result = { marker: "result" } as const;
        success.resolve(result);
        await expect(successful).resolves.toBe(result);
        expect(performanceTelemetry.snapshot()).toMatchObject({
            counters: { [IPC_INVOKE_SUCCESS_COUNTER]: 1 },
            gauges: { [IPC_INVOKE_ACTIVE_GAUGE]: 1 },
        });

        const nativeError = { category: "git", message: "sensitive native detail", payload: { secret: true } };
        failure.reject(nativeError);
        await expect(failed).rejects.toBe(nativeError);

        const snapshot = performanceTelemetry.snapshot();
        expect(snapshot).toMatchObject({
            counters: { [IPC_INVOKE_SUCCESS_COUNTER]: 1, [IPC_INVOKE_ERROR_COUNTER]: 1 },
            gauges: { [IPC_INVOKE_ACTIVE_GAUGE]: 0 },
            latencies: { [IPC_INVOKE_METRIC]: { count: 2, totalCount: 2 } },
        });
        const spans = snapshot.spans.filter((span) => span.name === IPC_INVOKE_METRIC);
        expect(spans).toHaveLength(2);
        expect(spans.map((span) => span.metadata.command)).toEqual(["files_call", "state_save"]);
        const requestIds = spans.map((span) => span.metadata.requestId as number);
        expect(requestIds[1]).toBe(requestIds[0] + 1);
        expect(spans[1].metadata).toMatchObject({ outcome: "error", category: "git" });
        expect(JSON.stringify(snapshot)).not.toContain("sensitive native detail");
        expect(JSON.stringify(snapshot)).not.toContain("secret");
    });

    it("passes opaque Channel-containing args and transport options through by identity", async () => {
        let payloadReads = 0;
        const channel = Object.freeze({ opaqueChannel: true });
        const args = Object.defineProperties(
            {},
            {
                channel: { enumerable: true, value: channel },
                privatePayload: {
                    enumerable: true,
                    get: () => {
                        payloadReads += 1;
                        throw new Error("adapter inspected args");
                    },
                },
            },
        ) as InvokeArgs;
        const invokeOptions: InvokeOptions = { headers: { "x-test": "opaque" } };
        const result = Object.defineProperty({}, "privateResult", {
            enumerable: true,
            get: () => {
                throw new Error("adapter inspected result");
            },
        });
        transport.register("pty_attach", (receivedArgs, context) => {
            expect(receivedArgs).toBe(args);
            expect(context.native).toBe(invokeOptions);
            expect((receivedArgs as Record<string, unknown>).channel).toBe(channel);
            return result;
        });

        await expect(invokeCommand("pty_attach", args, { invokeOptions })).resolves.toBe(result);

        expect(payloadReads).toBe(0);
        const encodedTelemetry = JSON.stringify(performanceTelemetry.snapshot());
        expect(encodedTelemetry).not.toContain("privatePayload");
        expect(encodedTelemetry).not.toContain("privateResult");
        expect(encodedTelemetry).not.toContain("opaqueChannel");
    });

    it("preserves cancellation reasons and observes late transport settlement", async () => {
        const pending = deferred<{ secretResult: string }>();
        const controller = new AbortController();
        const args = { channel: Object.freeze({ id: 7 }) };
        const handler = vi.fn((receivedArgs, context) => {
            expect(receivedArgs).toBe(args);
            expect(context.signal).toBe(controller.signal);
            return pending.promise;
        });
        transport.register("pty_sessions", handler);
        const invocation = invokeCommand("pty_sessions", args, { signal: controller.signal });
        const reason = Object.freeze({ privateCancellationReason: true });

        controller.abort(reason);
        await expect(invocation).rejects.toBe(reason);

        expect(handler).toHaveBeenCalledOnce();
        expect(performanceTelemetry.snapshot()).toMatchObject({
            counters: { [IPC_INVOKE_CANCEL_COUNTER]: 1 },
            gauges: { [IPC_INVOKE_ACTIVE_GAUGE]: 0 },
            latencies: { [IPC_INVOKE_METRIC]: { count: 1 } },
        });
        expect(JSON.stringify(performanceTelemetry.snapshot())).not.toContain("privateCancellationReason");

        pending.resolve({ secretResult: "not retained" });
        await Promise.resolve();
        expect(JSON.stringify(performanceTelemetry.snapshot())).not.toContain("not retained");
    });

    it("does not derive categories from ordinary errors and never reuses request IDs", async () => {
        const firstError = new Error("private failure detail");
        transport.register("boot_init", async () => {
            throw firstError;
        });
        await expect(invokeCommand("boot_init")).rejects.toBe(firstError);
        const firstRequestId = performanceTelemetry.snapshot().spans[0].metadata.requestId as number;

        performanceTelemetry.reset();
        transport.register("integration_health", async () => null);
        await invokeCommand<null>("integration_health");

        const snapshot = performanceTelemetry.snapshot();
        expect(snapshot.spans[0].metadata.requestId).toBeGreaterThan(firstRequestId);
        expect(snapshot.spans[0].metadata).toEqual({
            command: "integration_health",
            requestId: snapshot.spans[0].metadata.requestId,
            outcome: "success",
        });
        expect(JSON.stringify(snapshot)).not.toContain("private failure detail");
    });

    it("leaves no activity breadcrumb outstanding once a command settles", async () => {
        uiActivity.reset();
        const success = deferred<null>();
        const failure = deferred<never>();
        const cancelled = deferred<never>();
        const controller = new AbortController();
        transport.register("open_url", () => success.promise);
        transport.register("state_save", () => failure.promise);
        transport.register("pty_sessions", () => cancelled.promise);

        const resolving = invokeCommand<null>("open_url");
        const rejecting = invokeCommand<never>("state_save", { repo: "/project" });
        const aborting = invokeCommand<never>("pty_sessions", undefined, { signal: controller.signal });
        expect(uiActivity.snapshot().inflight.map((entry) => entry.command)).toEqual(["open_url", "state_save", "pty_sessions"]);

        success.resolve(null);
        await resolving;
        failure.reject(new Error("native failure detail"));
        await expect(rejecting).rejects.toThrow("native failure detail");
        controller.abort(new Error("cancelled"));
        await expect(aborting).rejects.toThrow("cancelled");

        const report = uiActivity.snapshot();
        expect(report.inflight).toEqual([]);
        expect(report.recent.map((entry) => ({ command: entry.command, ok: entry.ok }))).toEqual([
            { command: "pty_sessions", ok: false },
            { command: "state_save", ok: false },
            { command: "open_url", ok: true },
        ]);
        expect(JSON.stringify(report)).not.toContain("native failure detail");
        uiActivity.reset();
    });
});
