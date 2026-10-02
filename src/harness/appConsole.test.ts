import { describe, expect, it, vi } from "vitest";
import { AppConsole } from "./appConsole";

function fakeWindow() {
    const target = new EventTarget() as EventTarget & { console: Record<string, (...parts: unknown[]) => void> };
    target.console = { log: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn(), debug: vi.fn() };
    return target;
}

describe("app console", () => {
    it("keeps console calls, uncaught errors and swallowed errors in time order and still prints them", () => {
        const swallowed = [{ ts: Date.now() + 1000, label: "avatar fetch", err: new TypeError("Load failed") }];
        const recorder = new AppConsole(() => swallowed);
        const target = fakeWindow();
        const printed = target.console.warn;
        recorder.install(target as unknown as Window & typeof globalThis);
        target.console.log("hello", { id: 1 });
        target.console.warn("careful");
        target.dispatchEvent(Object.assign(new Event("error"), { error: new Error("boom"), filename: "app.js", lineno: 3, colno: 9 }));
        target.dispatchEvent(Object.assign(new Event("unhandledrejection"), { reason: "nope" }));
        expect(printed).toHaveBeenCalledWith("careful");
        const all = recorder.read({});
        expect(all.messages.map((entry) => [entry.level, entry.text.split("\n")[0]])).toEqual([
            ["log", 'hello {"id":1}'],
            ["warn", "careful"],
            ["uncaught", "Error: boom"],
            ["unhandled rejection", "nope"],
            ["swallowed", "avatar fetch: TypeError: Load failed"],
        ]);
        expect(all.messages[2].text).toContain("(app.js:3:9)");
        expect(all.messages[0].at).toMatch(/^\d{4}-\d\d-\d\dT/);
        const errors = recorder.read({ errors: true, limit: 2 });
        expect(errors).toMatchObject({ recorded: 5, matched: 4 });
        expect(errors.messages.map((entry) => entry.level)).toEqual(["unhandled rejection", "swallowed"]);
    });
    it("bounds what it keeps and rejects bad arguments", () => {
        const recorder = new AppConsole(() => []);
        for (let index = 0; index < 600; index++) recorder.add("log", [index]);
        recorder.add("log", [{ big: "x".repeat(10_000) }]);
        recorder.add("log", ["y".repeat(10_000)]);
        const { recorded, messages } = recorder.read({ limit: 200 });
        expect(recorded).toBe(500);
        expect(messages.at(-2)!.text).toBe("[object Object] (too large to show)");
        expect(messages.at(-1)!.text).toHaveLength(2001);
        expect(() => recorder.read({ limit: 0 })).toThrow("limit");
        expect(() => recorder.read({ errors: "yes" })).toThrow("errors");
    });
});
