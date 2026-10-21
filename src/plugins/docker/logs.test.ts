import { describe, expect, it } from "vitest";
import type { LogLine } from "./api";
import { appendLines, clockOf, filterLines, linesAsText } from "./logs";

const line = (text: string, stream: LogLine["stream"] = "stdout", at: string | null = null): LogLine => ({ at, stream, text });

describe("a followed log", () => {
    it("keeps the newest lines once it is full", () => {
        const kept = appendLines([line("a"), line("b")], [line("c"), line("d")], 3);
        expect(kept.map((each) => each.text)).toEqual(["b", "c", "d"]);
        const same = [line("a")];
        expect(appendLines(same, [])).toBe(same);
    });

    it("shows only the lines with the words asked for, or only stderr", () => {
        const lines = [line("GET / 200"), line("ERROR db refused", "stderr"), line("warning: slow", "stderr")];
        expect(filterLines(lines, { query: "error", errorsOnly: false }).map((each) => each.text)).toEqual(["ERROR db refused"]);
        expect(filterLines(lines, { query: "", errorsOnly: true })).toHaveLength(2);
        expect(filterLines(lines, { query: "  ", errorsOnly: false })).toBe(lines);
    });

    it("reads the engine's timestamps to the millisecond, and copies lines with them when asked", () => {
        expect(clockOf("2026-10-18T10:00:01.123456789Z")).toMatch(/:01\.123$/);
        expect(clockOf("not a time")).toBe("");
        expect(clockOf(null)).toBe("");
        const lines = [line("one", "stdout", "2026-10-18T10:00:01Z"), line("two")];
        expect(linesAsText(lines, true)).toBe("2026-10-18T10:00:01Z one\ntwo");
        expect(linesAsText(lines, false)).toBe("one\ntwo");
    });
});
