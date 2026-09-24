import { beforeEach, describe, expect, it } from "vitest";
import { applyChatTextScale, clampChatTextScale, DEFAULT_CHAT_TEXT_SCALE } from "./textScale";

describe("chat text scale", () => {
    beforeEach(() => {
        document.documentElement.style.removeProperty("--chat-text-scale");
    });

    it("keeps the scale inside a readable range", () => {
        expect(clampChatTextScale(0.2)).toBe(0.7);
        expect(clampChatTextScale(9)).toBe(2);
        expect(clampChatTextScale(Number.NaN)).toBe(DEFAULT_CHAT_TEXT_SCALE);
    });

    it("rounds away the drift that repeated steps accumulate", () => {
        expect(clampChatTextScale(1 + 0.1 + 0.1 + 0.1)).toBe(1.3);
    });

    it("publishes the scale as the variable the transcript multiplies by", () => {
        applyChatTextScale(1.4);

        expect(document.documentElement.style.getPropertyValue("--chat-text-scale")).toBe("1.4");
    });

    it("never publishes a value outside the range", () => {
        applyChatTextScale(99);

        expect(document.documentElement.style.getPropertyValue("--chat-text-scale")).toBe("2");
    });
});
