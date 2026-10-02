import { describe, expect, it } from "vitest";
import type { AcpEvent } from "../api/acp";
import { afterSessionEnd, RESUME_WINDOW_MS, sessionEndOf } from "./sessionRecovery";

const status = (payload: Record<string, unknown>): AcpEvent => ({ agentId: "a1", kind: "status", payload });

describe("sessionEndOf", () => {
    it("reads why a session ended", () => {
        expect(sessionEndOf(status({ state: "stopped", reason: "requested" }))).toBe("requested");
        expect(sessionEndOf(status({ state: "stopped", reason: "exited" }))).toBe("exited");
        expect(sessionEndOf(status({ state: "error", reason: "failed" }))).toBe("failed");
    });

    it("says nothing for a status that is not an end", () => {
        expect(sessionEndOf(status({ state: "starting" }))).toBeNull();
        expect(sessionEndOf(status({ state: "stopped", reason: "bored" }))).toBeNull();
    });
});

describe("afterSessionEnd", () => {
    const base = { end: "exited" as const, resumeId: "session-1", resuming: false, lastResumeAt: null, now: 100_000 };

    it("resumes an agent that died with a saved session", () => {
        expect(afterSessionEnd(base)).toBe("resume");
    });

    it("never resumes a stop that was asked for", () => {
        expect(afterSessionEnd({ ...base, end: "requested" })).toBe("leave");
        expect(afterSessionEnd({ ...base, end: "requested", resuming: true })).toBe("leave");
    });

    it("gives up on an agent with no saved session to resume", () => {
        expect(afterSessionEnd({ ...base, resumeId: undefined })).toBe("give-up");
    });

    it("gives up when a resume fails to start or dies right away", () => {
        expect(afterSessionEnd({ ...base, end: "failed", resuming: true })).toBe("give-up");
        expect(afterSessionEnd({ ...base, resuming: true })).toBe("give-up");
    });

    it("gives up on a second death soon after the last resume, and resumes once the window passes", () => {
        expect(afterSessionEnd({ ...base, lastResumeAt: base.now - RESUME_WINDOW_MS + 1 })).toBe("give-up");
        expect(afterSessionEnd({ ...base, lastResumeAt: base.now - RESUME_WINDOW_MS })).toBe("resume");
    });

    it("leaves a first start that failed to the person", () => {
        expect(afterSessionEnd({ ...base, end: "failed" })).toBe("leave");
    });
});
