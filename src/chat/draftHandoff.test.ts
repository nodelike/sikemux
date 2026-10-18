import { describe, expect, it } from "vitest";
import { putDraft, registerComposer, takeDraft } from "./draftHandoff";

describe("handing a draft between an agent's views", () => {
    it("takes the composer's draft, emptying it, and puts text back after what is there", () => {
        let draft = "ten lines\\nof thought";
        const stop = registerComposer("agent-1", {
            take: () => {
                const taken = draft;
                draft = "";
                return taken;
            },
            put: (text) => {
                draft = draft ? `${draft}\\n${text}` : text;
            },
        });
        expect(takeDraft("agent-1")).toBe("ten lines\\nof thought");
        expect(draft).toBe("");
        expect(putDraft("agent-1", "typed in the TUI")).toBe(true);
        expect(putDraft("agent-1", "   ")).toBe(false);
        expect(draft).toBe("typed in the TUI");
        stop();
        expect(takeDraft("agent-1")).toBe("");
        expect(putDraft("agent-1", "nowhere to go")).toBe(false);
    });
});
