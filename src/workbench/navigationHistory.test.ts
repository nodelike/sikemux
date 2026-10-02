import { describe, expect, it, vi } from "vitest";
import { NAVIGATION_HISTORY_LIMITS, NavigationHistory, parseNavigationLocation, type NavigationLocationInput } from "./navigationHistory";

function location(path: string, overrides: Partial<NavigationLocationInput> = {}): NavigationLocationInput {
    return { project: "/repo", path, ...overrides };
}

function paths(locations: readonly NavigationLocationInput[]): string[] {
    return locations.map(({ path }) => path);
}

describe("NavigationHistory locations", () => {
    it("clones and deeply isolates immutable content-free locations", () => {
        const history = new NavigationHistory();
        const mutable = {
            project: "/repo",
            path: "/repo/a.ts",
            line: 4,
            column: 2,
            symbol: "run",
        };

        expect(history.push(mutable)).toBe("pushed");
        mutable.path = "/repo/changed.ts";
        mutable.line = 99;
        const snapshot = history.getSnapshot();

        expect(snapshot.current).toEqual({ project: "/repo", path: "/repo/a.ts", line: 4, column: 2, symbol: "run" });
        expect(Object.isFrozen(snapshot)).toBe(true);
        expect(Object.isFrozen(snapshot.current)).toBe(true);
        expect(Object.isFrozen(snapshot.backward)).toBe(true);
        expect(Object.isFrozen(snapshot.forward)).toBe(true);
    });

    it.each([
        ["non-object", null],
        ["array", []],
        ["missing project", { path: "/repo/a.ts" }],
        ["blank project", { project: "  ", path: "/repo/a.ts" }],
        ["control character", { project: "/repo", path: "/repo/a\u0000.ts" }],
        ["oversized path", { project: "/repo", path: "a".repeat(NAVIGATION_HISTORY_LIMITS.maxPathBytes + 1) }],
        ["oversized multibyte path", { project: "/repo", path: "é".repeat(NAVIGATION_HISTORY_LIMITS.maxPathBytes / 2 + 1) }],
        ["negative line", { project: "/repo", path: "/repo/a.ts", line: -1 }],
        ["fractional line", { project: "/repo", path: "/repo/a.ts", line: 1.5 }],
        ["oversized column", { project: "/repo", path: "/repo/a.ts", line: 1, column: NAVIGATION_HISTORY_LIMITS.maxPosition + 1 }],
        ["column without line", { project: "/repo", path: "/repo/a.ts", column: 1 }],
        ["blank symbol", { project: "/repo", path: "/repo/a.ts", symbol: " " }],
        ["untrimmed symbol", { project: "/repo", path: "/repo/a.ts", symbol: " run" }],
        ["unknown payload field", { project: "/repo", path: "/repo/a.ts", contents: "never retain me" }],
    ])("rejects malformed locations: %s", (_label, value) => {
        expect(parseNavigationLocation(value)).toBeNull();
    });

    it("rejects accessors and proxy failures without reading arbitrary payloads", () => {
        const getter = vi.fn(() => "/repo/a.ts");
        const accessor = { project: "/repo" };
        Object.defineProperty(accessor, "path", { enumerable: true, get: getter });
        const hostile = new Proxy(
            {},
            {
                ownKeys: () => {
                    throw new Error("hostile proxy");
                },
            },
        );

        expect(parseNavigationLocation(accessor)).toBeNull();
        expect(getter).not.toHaveBeenCalled();
        expect(parseNavigationLocation(hostile)).toBeNull();
    });

    it("rejects invalid pushes without mutating history", () => {
        const history = new NavigationHistory();
        history.push(location("/repo/a.ts"));

        expect(history.push({ project: "/repo", path: "" })).toBe("invalid");
        expect(history.getSnapshot()).toMatchObject({ size: 1, current: { path: "/repo/a.ts" } });
    });
});

describe("NavigationHistory traversal", () => {
    it("deduplicates consecutive locations and traverses both stacks", () => {
        const history = new NavigationHistory();
        expect(history.push(location("/repo/a.ts"))).toBe("pushed");
        expect(history.push(location("/repo/b.ts", { line: 7 }))).toBe("pushed");
        expect(history.push(location("/repo/b.ts", { line: 7 }))).toBe("duplicate");
        expect(history.push(location("/repo/c.ts"))).toBe("pushed");
        expect(paths(history.getSnapshot().backward)).toEqual(["/repo/a.ts", "/repo/b.ts"]);

        expect(history.back()).toMatchObject({ path: "/repo/b.ts", line: 7 });
        expect(history.getSnapshot()).toMatchObject({ canGoBack: true, canGoForward: true });
        expect(history.back()).toMatchObject({ path: "/repo/a.ts" });
        expect(history.back()).toBeNull();
        expect(history.forward()).toMatchObject({ path: "/repo/b.ts", line: 7 });
        expect(history.forward()).toMatchObject({ path: "/repo/c.ts" });
        expect(history.forward()).toBeNull();
    });

    it("preserves the forward branch for a duplicate but truncates it for a new push", () => {
        const history = new NavigationHistory();
        history.push(location("/repo/a.ts"));
        history.push(location("/repo/b.ts"));
        history.push(location("/repo/c.ts"));
        expect(history.back()).toMatchObject({ path: "/repo/b.ts" });

        expect(history.push(location("/repo/b.ts"))).toBe("duplicate");
        expect(paths(history.getSnapshot().forward)).toEqual(["/repo/c.ts"]);
        expect(history.push(location("/repo/d.ts"))).toBe("pushed");

        const snapshot = history.getSnapshot();
        expect(paths(snapshot.backward)).toEqual(["/repo/a.ts", "/repo/b.ts"]);
        expect(snapshot.current).toMatchObject({ path: "/repo/d.ts" });
        expect(snapshot.forward).toEqual([]);
        expect(snapshot.canGoForward).toBe(false);
    });

    it("bounds total retained locations and evicts the oldest back entries", () => {
        expect(() => new NavigationHistory({ capacity: 0 })).toThrow(RangeError);
        expect(() => new NavigationHistory({ capacity: NAVIGATION_HISTORY_LIMITS.maxCapacity + 1 })).toThrow(RangeError);
        const history = new NavigationHistory({ capacity: 3 });
        for (const name of ["a", "b", "c", "d", "e"]) history.push(location(`/repo/${name}.ts`));

        const snapshot = history.getSnapshot();
        expect(snapshot).toMatchObject({ capacity: 3, size: 3, current: { path: "/repo/e.ts" } });
        expect(paths(snapshot.backward)).toEqual(["/repo/c.ts", "/repo/d.ts"]);
        expect(history.back()).toMatchObject({ path: "/repo/d.ts" });
        expect(history.back()).toMatchObject({ path: "/repo/c.ts" });
        expect(history.back()).toBeNull();
    });

    it("rejects stale pushes and lazily prunes stale traversal targets", () => {
        const currentPaths = new Set(["/repo/a.ts", "/repo/b.ts", "/repo/c.ts"]);
        const history = new NavigationHistory({
            isLocationCurrent: ({ path }) => {
                if (path === "/repo/throws.ts") throw new Error("project lookup failed");
                return currentPaths.has(path);
            },
        });
        history.push(location("/repo/a.ts"));
        history.push(location("/repo/b.ts"));
        history.push(location("/repo/c.ts"));

        currentPaths.delete("/repo/b.ts");
        expect(history.back()).toMatchObject({ path: "/repo/a.ts" });
        expect(paths(history.getSnapshot().forward)).toEqual(["/repo/c.ts"]);

        currentPaths.delete("/repo/c.ts");
        expect(history.forward()).toBeNull();
        expect(history.getSnapshot()).toMatchObject({ size: 1, current: { path: "/repo/a.ts" }, canGoForward: false });
        expect(history.push(location("/repo/missing.ts"))).toBe("stale");
        expect(history.push(location("/repo/throws.ts"))).toBe("stale");
        expect(history.getSnapshot()).toMatchObject({ size: 1, current: { path: "/repo/a.ts" } });
    });
});
