import { recordOf } from "../lib/guards";
import type { AcpEvent } from "../api/acp";
import type { AcpPermissionRequest, ChatAction, ChatFailure } from "./types";

type ChatEvent = Pick<AcpEvent, "payload"> & { kind: string };

const atOf = (payload: Record<string, unknown>) => (typeof payload.at === "number" ? payload.at : undefined);

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

export function statusFromEvent(event: Pick<AcpEvent, "payload">): "connecting" | "starting" | "initializing" | "ready" | "stopped" | "error" {
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
export function failureOf(event: Pick<AcpEvent, "payload">): ChatFailure | undefined {
    const failure = recordOf(event.payload.failure);
    const kind = failure?.kind;
    if (kind !== "signIn" && kind !== "limit" && kind !== "network" && kind !== "other") return undefined;
    const details = typeof failure?.details === "string" && failure.details ? { details: failure.details } : {};
    return { kind, account: typeof failure?.account === "string" ? failure.account : null, ...details };
}

/** What the chat's reducer makes of an event from the core, on the Mac and the phone alike. */
export function chatActions(event: ChatEvent): ChatAction[] {
    const payload = event.payload;
    switch (event.kind) {
        case "status":
            return [{ type: "status", state: statusFromEvent(event) }];
        case "ready":
            return [{ type: "ready", capabilities: recordOf(payload.capabilities) ?? {}, setup: recordOf(payload.setup) ?? {} }];
        case "session_update": {
            const rows = Array.isArray(payload.updates) ? payload.updates : [payload];
            return rows.flatMap((entry): ChatAction[] => {
                const row = recordOf(entry);
                const update = row && recordOf(row.update);
                return update && typeof row.sessionId === "string" ? [{ type: "session_update", sessionId: row.sessionId, update }] : [];
            });
        }
        case "prompt": {
            const prompted = promptAction(payload);
            return prompted ? [prompted] : [];
        }
        case "turn_started":
            return [{ type: "turn_started", at: atOf(payload) }];
        case "turn_completed":
            return [
                { type: "turn_completed", stopReason: typeof payload.stopReason === "string" ? payload.stopReason : undefined, at: atOf(payload) },
            ];
        case "permission_request": {
            const request = permissionRequest(payload);
            return request ? [{ type: "permission_requested", request }] : [];
        }
        case "permission_cleared":
            return typeof payload.requestId === "string" ? [{ type: "permission_cleared", requestId: payload.requestId }] : [];
        case "error":
            return [
                { type: "error", message: typeof payload.message === "string" ? payload.message : "The agent stopped.", failure: failureOf(event) },
            ];
        default:
            return [];
    }
}
