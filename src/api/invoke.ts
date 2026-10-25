import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { uiActivity } from "../lib/activity";
import { performanceTelemetry } from "../lib/performance";
import { errorCategory } from "@shared/lib/format";
import { getIpcTransport } from "./transport";
import type { IpcCommand } from "./generated/ipcCommands";

export const IPC_INVOKE_METRIC = "ipc.invoke";
export const IPC_INVOKE_SUCCESS_COUNTER = "ipc.invoke.success";
export const IPC_INVOKE_ERROR_COUNTER = "ipc.invoke.error";
export const IPC_INVOKE_CANCEL_COUNTER = "ipc.invoke.cancel";
export const IPC_INVOKE_ACTIVE_GAUGE = "ipc.invoke.active";

export interface InvokeCommandOptions {
    readonly signal?: AbortSignal;
    readonly invokeOptions?: InvokeOptions;
}

type InvokeOutcome<T> =
    | { readonly kind: "success"; readonly value: T }
    | { readonly kind: "error"; readonly error: unknown }
    | { readonly kind: "cancel"; readonly reason: unknown };

let requestSequence = 0;
let activeInvocations = 0;

function settleInvocation<T>(command: string, args: InvokeArgs | undefined, options: InvokeCommandOptions | undefined): Promise<InvokeOutcome<T>> {
    const signal = options?.signal;
    return getIpcTransport()
        .invoke<T>(command, args, { signal, native: options?.invokeOptions })
        .then(
            (value): InvokeOutcome<T> => ({ kind: "success", value }),
            (error: unknown): InvokeOutcome<T> => (signal?.aborted ? { kind: "cancel", reason: signal.reason } : { kind: "error", error }),
        );
}

/**
 * Invoke a native command while retaining only its name, request ID, duration,
 * and outcome. Arguments, return values, and error details remain opaque.
 */
export async function invokeCommand<T>(command: IpcCommand, args?: InvokeArgs, options?: InvokeCommandOptions): Promise<T> {
    requestSequence += 1;
    const requestId = requestSequence;
    const span = performanceTelemetry.startTrace(IPC_INVOKE_METRIC, { command, requestId });
    const ticket = uiActivity.beginCommand(command);
    activeInvocations += 1;
    performanceTelemetry.setGauge(IPC_INVOKE_ACTIVE_GAUGE, activeInvocations);

    const outcome = await settleInvocation<T>(command, args, options);
    activeInvocations -= 1;
    performanceTelemetry.setGauge(IPC_INVOKE_ACTIVE_GAUGE, activeInvocations);
    uiActivity.endCommand(ticket, outcome.kind === "success");

    if (outcome.kind === "success") {
        performanceTelemetry.incrementCounter(IPC_INVOKE_SUCCESS_COUNTER);
        const recorded = performanceTelemetry.endSpan(span, { outcome: "success" });
        if (recorded) performanceTelemetry.recordLatency(IPC_INVOKE_METRIC, recorded.durationMs);
        return outcome.value;
    }

    if (outcome.kind === "cancel") {
        performanceTelemetry.incrementCounter(IPC_INVOKE_CANCEL_COUNTER);
        const recorded = performanceTelemetry.endSpan(span, { outcome: "cancel" });
        if (recorded) performanceTelemetry.recordLatency(IPC_INVOKE_METRIC, recorded.durationMs);
        throw outcome.reason;
    }

    performanceTelemetry.incrementCounter(IPC_INVOKE_ERROR_COUNTER);
    const category = errorCategory(outcome.error);
    const recorded = performanceTelemetry.endSpan(span, category ? { outcome: "error", category } : { outcome: "error" });
    if (recorded) performanceTelemetry.recordLatency(IPC_INVOKE_METRIC, recorded.durationMs);
    throw outcome.error;
}
