import type { AcpEvent } from "../api/acp";

/* Why the backend says a session ended: a stop the app asked for, an agent
   that died after it was up, or a start that never got there. */
export type SessionEnd = "requested" | "exited" | "failed";

export type Recovery = { phase: "resuming" } | { phase: "failed"; detail: string | null };

export function sessionEndOf(event: AcpEvent): SessionEnd | null {
    const reason = event.payload.reason;
    return reason === "requested" || reason === "exited" || reason === "failed" ? reason : null;
}

/* The core starts an agent that dies under the chat again by itself until it
   has died too often, so a death that still reaches the app is one the core
   gave up on, and the choice goes to the person. */
export function afterSessionEnd(end: SessionEnd): "give-up" | "leave" {
    return end === "exited" ? "give-up" : "leave";
}
