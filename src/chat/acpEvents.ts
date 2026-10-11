import type { AcpEvent } from "../api/acp";
import type { AcpPermissionRequest, ChatAction, ChatFailure } from "./types";

export function recordOf(value: unknown): Record<string, unknown> | null {
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

export function eventMessage(event: AcpEvent): string {
    return typeof event.payload.message === "string" ? event.payload.message : "ACP session failed";
}

export function permissionRequest(payload: Record<string, unknown>): AcpPermissionRequest | null {
    const requestId = typeof payload.requestId === "string" ? payload.requestId : null;
    const sessionId = typeof payload.sessionId === "string" ? payload.sessionId : null;
    const toolCall = recordOf(payload.toolCall);
    const options = Array.isArray(payload.options) ? payload.options : null;
    if (!requestId || !sessionId || !toolCall || typeof toolCall.toolCallId !== "string" || !options) return null;
    return {
        requestId,
        sessionId,
        toolCall: { ...toolCall, toolCallId: toolCall.toolCallId, title: typeof toolCall.title === "string" ? toolCall.title : "Agent tool" },
        options: options.flatMap((option) => {
            const row = recordOf(option);
            return row && typeof row.optionId === "string" && typeof row.name === "string" && typeof row.kind === "string"
                ? [{ optionId: row.optionId, name: row.name, kind: row.kind }]
                : [];
        }),
    };
}

export function statusFromEvent(event: AcpEvent): "connecting" | "starting" | "initializing" | "ready" | "stopped" | "error" {
    const value = event.payload.state;
    return value === "starting" || value === "initializing" || value === "ready" || value === "stopped" || value === "error" ? value : "connecting";
}

/** A prompt sent from another device, shown as if it had been typed here. */
export function promptAction(payload: Record<string, unknown>): ChatAction | null {
    if (typeof payload.text !== "string") return null;
    const paths = Array.isArray(payload.paths) ? payload.paths.filter((path): path is string => typeof path === "string") : [];
    return {
        type: "local_prompt",
        text: payload.text,
        paths,
        ...(typeof payload.messageId === "string" ? { messageId: payload.messageId } : {}),
        ...(typeof payload.at === "number" ? { at: payload.at } : {}),
    };
}

/** Why a turn failed, when the core says it was the account's doing. */
export function failureOf(event: AcpEvent): ChatFailure | undefined {
    const failure = recordOf(event.payload.failure);
    const kind = failure?.kind;
    if (kind !== "signIn" && kind !== "limit" && kind !== "other") return undefined;
    return { kind, account: typeof failure?.account === "string" ? failure.account : null };
}
