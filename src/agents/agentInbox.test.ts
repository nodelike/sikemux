import { describe, expect, it, vi } from "vitest";
import { deliverToAgent, receiveForAgent } from "./agentInbox";

describe("agent inbox", () => {
    it("hands a delivery straight to the input on screen", () => {
        const receiver = vi.fn();
        const stop = receiveForAgent("a1", receiver);
        deliverToAgent("a1", { text: "look at this" });
        expect(receiver).toHaveBeenCalledWith({ text: "look at this" });
        stop();
    });

    it("keeps deliveries for an agent off screen until its input appears", () => {
        deliverToAgent("a2", { text: "first" });
        deliverToAgent("a2", { paths: ["/repo/a.ts"] });
        const receiver = vi.fn();
        const stop = receiveForAgent("a2", receiver);
        expect(receiver.mock.calls).toEqual([[{ text: "first" }], [{ paths: ["/repo/a.ts"] }]]);
        stop();
        deliverToAgent("a2", { text: "later" });
        const next = vi.fn();
        receiveForAgent("a2", next)();
        expect(next).toHaveBeenCalledWith({ text: "later" });
    });
});
