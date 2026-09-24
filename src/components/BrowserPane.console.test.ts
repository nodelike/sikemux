import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/* The same injected recorder that watches a tab's requests also keeps what
   the page logs, so an agent can read errors without opening devtools. */
const RECORDER = readFileSync(resolve(__dirname, "../../src-tauri/src/browser/recorder.js"), "utf8");

interface Message {
    level: string;
    text: string;
}

const messages = () => (window as unknown as { __sikemuxConsole: { entries: () => Message[] } }).__sikemuxConsole.entries();
const originals = { ...console };

beforeEach(() => {
    for (const level of ["log", "info", "warn", "error", "debug"] as const) console[level] = vi.fn();
    delete (window as unknown as Record<string, unknown>).__sikemuxConsole;
    (0, eval)(RECORDER);
});

afterEach(() => {
    Object.assign(console, originals);
});

describe("browser console recorder", () => {
    it("keeps each message with its level and still hands it to the real console", () => {
        console.error("save failed", { code: 7 });
        console.log("ready");

        expect(messages()).toMatchObject([
            { level: "error", text: 'save failed {"code":7}' },
            { level: "log", text: "ready" },
        ]);
    });

    it("spells out an error, which WebKit's stack alone leaves unnamed", () => {
        console.warn(new TypeError("plan is undefined"));

        expect(messages()[0].text.startsWith("TypeError: plan is undefined")).toBe(true);
    });

    it("records errors and rejections nobody caught", () => {
        window.dispatchEvent(new ErrorEvent("error", { error: new Error("boom"), message: "boom" }));
        const rejection = new Event("unhandledrejection") as Event & { reason?: unknown };
        rejection.reason = "denied";
        window.dispatchEvent(rejection);

        expect(messages().map((message) => [message.level, message.text.split("\n")[0]])).toEqual([
            ["uncaught", "Error: boom"],
            ["unhandled rejection", "denied"],
        ]);
    });

    it("holds a bounded history", () => {
        for (let index = 0; index < 210; index += 1) console.info(`line ${index}`);

        expect(messages()).toHaveLength(200);
        expect(messages()[0].text).toBe("line 10");
    });
});
