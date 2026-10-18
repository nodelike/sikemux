import { describe, expect, it } from "vitest";
import type { AcpEvent } from "../api/acp";
import { afterSessionEnd, sessionEndOf } from "./sessionRecovery";

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
    it("hands an agent the core gave up starting again to the person", () => {
        expect(afterSessionEnd("exited")).toBe("give-up");
    });

    it("leaves a stop that was asked for, and a start that failed", () => {
        expect(afterSessionEnd("requested")).toBe("leave");
        expect(afterSessionEnd("failed")).toBe("leave");
    });
});
