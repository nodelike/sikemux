import { describe, expect, it } from "vitest";
import type { Frame } from "./api";
import { ago, count, groupFrames, levelTone, projectCandidates } from "./format";

const frame = (file: string, inApp: boolean): Frame => ({ file, module: null, function: "f", line: 1, column: null, inApp, context: [] });

describe("sentry words", () => {
    it("writes counts and times as Sentry does", () => {
        expect(count(950)).toBe("950");
        expect(count(1284)).toBe("1.3k");
        expect(count(42_310)).toBe("42k");
        expect(count(1_200_000)).toBe("1.2m");
        const now = Date.parse("2026-10-19T12:00:00Z");
        expect(ago("2026-10-19T11:59:15Z", now)).toBe("45s");
        expect(ago("2026-10-19T09:00:00Z", now)).toBe("3h");
        expect(ago("2026-10-13T12:00:00Z", now)).toBe("6d");
        expect(ago(null, now)).toBe("—");
    });

    it("colours levels by how bad they are", () => {
        expect(levelTone("fatal")).toBe("danger");
        expect(levelTone("warning")).toBe("warn");
        expect(levelTone("debug")).toBe("idle");
    });

    it("keeps the app's frames and folds library ones that sit together", () => {
        const groups = groupFrames([frame("a.ts", true), frame("lib1.js", false), frame("lib2.js", false), frame("b.ts", true)]);
        expect(groups.map((group) => group.kind)).toEqual(["frame", "library", "frame"]);
        expect(groupFrames([frame("lib.js", false)]).map((group) => group.kind)).toEqual(["frame"]);
    });

    it("finds where a frame's file could be in the project", () => {
        expect(projectCandidates("app:///src/checkout/summary.ts", "/Users/me/shop")).toEqual([
            "/Users/me/shop/src/checkout/summary.ts",
            "/Users/me/shop/checkout/summary.ts",
            "/Users/me/shop/summary.ts",
        ]);
        expect(projectCandidates("webpack-internal:///./app/page.tsx?abc", "/w")).toEqual(["/w/app/page.tsx", "/w/page.tsx"]);
        expect(projectCandidates("/Users/me/shop/api/views.py", "/Users/me/shop")).toEqual(["/Users/me/shop/api/views.py"]);
        expect(projectCandidates("node_modules/react-dom/index.js", "/w")).toEqual([]);
        expect(projectCandidates("<anonymous>", "/w")).toEqual([]);
    });
});
