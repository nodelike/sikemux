import type { AcpEvent } from "../api/acp";

/* Why the backend says a session ended: a stop the app asked for, an agent
   that died after it was up, or a start that never got there. */
export type SessionEnd = "requested" | "exited" | "failed";

export const RESUME_WINDOW_MS = 30_000;

export type Recovery = { phase: "resuming" } | { phase: "failed"; detail: string | null };

export function sessionEndOf(event: AcpEvent): SessionEnd | null {
    const reason = event.payload.reason;
    return reason === "requested" || reason === "exited" || reason === "failed" ? reason : null;
}

/* A second death soon after a resume is a crash loop, and a resume that fails
   to start will fail the same way again, so both hand the choice to the person. */
export function afterSessionEnd({
    end,
    resumeId,
    resuming,
    lastResumeAt,
    now,
}: {
    end: SessionEnd;
    resumeId: string | undefined;
    resuming: boolean;
    lastResumeAt: number | null;
    now: number;
}): "resume" | "give-up" | "leave" {
    if (end === "requested") return "leave";
    if (resuming) return "give-up";
    if (end === "failed") return "leave";
    if (!resumeId) return "give-up";
    if (lastResumeAt !== null && now - lastResumeAt < RESUME_WINDOW_MS) return "give-up";
    return "resume";
}
