import { describe, expect, it } from "vitest";
import { agentCanStart } from "./agentStatus";

describe("agentStatus", () => {
    it("lets a signed-out agent start, since it asks to sign in, but not a missing or broken one", () => {
        expect(agentCanStart({ state: "signedOut" })).toBe(true);
        expect(agentCanStart({ state: "unknown" })).toBe(true);
        expect(agentCanStart(undefined)).toBe(true);
        expect(agentCanStart({ state: "missing" })).toBe(false);
        expect(agentCanStart({ state: "broken", reason: "x" })).toBe(false);
    });
});
