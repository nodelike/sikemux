import { describe, expect, it } from "vitest";
import { guessClaudeWindow } from "./contextWindow";

const models = (currentValue: string, options: { value: string; name: string }[]) => ({
    configOptions: [{ id: "model", type: "select", currentValue, options }],
});

describe("guessClaudeWindow", () => {
    it("gives a model named for a million tokens the long window", () => {
        expect(guessClaudeWindow(models("opus[1m]", []))).toBe(1_000_000);
        expect(guessClaudeWindow(models("default", [{ value: "default", name: "Opus (1M context)" }]))).toBe(1_000_000);
    });

    it("falls back to the model the chat was launched with", () => {
        expect(guessClaudeWindow({}, "sonnet[1m]")).toBe(1_000_000);
        expect(guessClaudeWindow({}, "sonnet")).toBe(200_000);
    });

    it("gives a session already past the standard window the long one", () => {
        expect(guessClaudeWindow({}, "claude-opus-5-5", 515_222)).toBe(1_000_000);
        expect(guessClaudeWindow({}, "claude-opus-5-5", 150_000)).toBe(200_000);
    });

    it("gives every other model the standard window", () => {
        expect(guessClaudeWindow(models("sonnet", [{ value: "sonnet", name: "Sonnet 5" }]))).toBe(200_000);
        expect(guessClaudeWindow({})).toBe(200_000);
        expect(guessClaudeWindow(models("claude-1mini", []))).toBe(200_000);
    });
});
