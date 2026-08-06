import { describe, expect, it } from "vitest";
import { parseSessionBundle, SESSION_BUNDLE_LIMITS } from "./sessionBundle";

function pane(id = "pane-1") {
    return { type: "pane", id, cwd: "/work", kind: "terminal", title: "shell" };
}

function bundle(overrides: Record<string, unknown> = {}) {
    return {
        format: "sikemux-session",
        version: 1,
        session: { id: "old-session", name: "demo", cwd: "/work", kind: "project" },
        windows: [{ id: "window-1", name: "1", role: "term", root: pane(), activePaneId: "pane-1" }],
        agents: [{ type: "codex", title: "Codex", resumeId: "resume-1" }],
        ...overrides,
    };
}

describe("session clipboard bundle validation", () => {
    it("accepts a complete v1 bundle", () => {
        expect(parseSessionBundle(JSON.stringify(bundle()))).toMatchObject({
            session: { name: "demo", cwd: "/work", kind: "project" },
            windows: [{ role: "term", activePaneId: "pane-1" }],
            agents: [{ type: "codex", resumeId: "resume-1" }],
        });
    });

    it.each([
        ["future version", { version: 2 }, "version is unsupported"],
        ["unsupported session kind", { session: { name: "demo", cwd: "/work", kind: "docker" } }, "kind is unsupported"],
        [
            "unsupported window role",
            { windows: [{ id: "window-1", name: "1", role: "browser", root: pane(), activePaneId: "pane-1" }] },
            "invalid or unsupported",
        ],
        [
            "unsupported pane kind",
            { windows: [{ id: "window-1", name: "1", role: "term", root: { ...pane(), kind: "browser" }, activePaneId: "pane-1" }] },
            "invalid or unsupported",
        ],
        [
            "missing active pane",
            { windows: [{ id: "window-1", name: "1", role: "term", root: pane(), activePaneId: "pane-missing" }] },
            "invalid or unsupported",
        ],
    ])("rejects %s", (_label, overrides, message) => {
        expect(() => parseSessionBundle(JSON.stringify(bundle(overrides)))).toThrow(message as string);
    });

    it("rejects duplicate window, layout, and agent claims", () => {
        expect(() =>
            parseSessionBundle(
                JSON.stringify(
                    bundle({
                        windows: [
                            { id: "same", name: "1", role: "term", root: pane("pane-1"), activePaneId: "pane-1" },
                            { id: "same", name: "2", role: "term", root: pane("pane-2"), activePaneId: "pane-2" },
                        ],
                    }),
                ),
            ),
        ).toThrow("duplicate window id");
        expect(() =>
            parseSessionBundle(
                JSON.stringify(
                    bundle({
                        windows: [
                            { id: "window-1", name: "1", role: "term", root: pane("same-pane"), activePaneId: "same-pane" },
                            { id: "window-2", name: "2", role: "term", root: pane("same-pane"), activePaneId: "same-pane" },
                        ],
                    }),
                ),
            ),
        ).toThrow("duplicate layout id across windows");
        expect(() =>
            parseSessionBundle(
                JSON.stringify(
                    bundle({
                        agents: [
                            { type: "codex", title: "one", resumeId: "same" },
                            { type: "codex", title: "two", resumeId: "same" },
                        ],
                    }),
                ),
            ),
        ).toThrow("duplicate agent session claim");
    });

    it("enforces depth, node-count, and clipboard-size limits", () => {
        let root: Record<string, unknown> = pane("deep-pane");
        for (let depth = 0; depth < SESSION_BUNDLE_LIMITS.maxDepth; depth++) {
            root = { type: "split", id: `split-${depth}`, dir: "row", children: [root], sizes: [1] };
        }
        expect(() =>
            parseSessionBundle(JSON.stringify(bundle({ windows: [{ id: "window-1", name: "1", role: "term", root, activePaneId: "deep-pane" }] }))),
        ).toThrow("invalid or unsupported");

        const children = Array.from({ length: SESSION_BUNDLE_LIMITS.maxChildren }, (_, index) => pane(`pane-${index}`));
        const wide = { type: "split", id: "wide", dir: "row", children, sizes: children.map(() => 1 / children.length) };
        expect(
            parseSessionBundle(JSON.stringify(bundle({ windows: [{ id: "window-1", name: "1", role: "term", root: wide, activePaneId: "pane-0" }] })))
                .windows,
        ).toHaveLength(1);

        const manyWindows = Array.from({ length: 32 }, (_, windowIndex) => {
            const windowChildren = Array.from({ length: 64 }, (_, paneIndex) => pane(`pane-${windowIndex}-${paneIndex}`));
            return {
                id: `window-${windowIndex}`,
                name: String(windowIndex + 1),
                role: "term",
                root: {
                    type: "split",
                    id: `root-${windowIndex}`,
                    dir: "row",
                    children: windowChildren,
                    sizes: windowChildren.map(() => 1 / windowChildren.length),
                },
                activePaneId: `pane-${windowIndex}-0`,
            };
        });
        expect(() => parseSessionBundle(JSON.stringify(bundle({ windows: manyWindows })))).toThrow("total layout node limit");

        expect(() => parseSessionBundle(" ".repeat(SESSION_BUNDLE_LIMITS.maxBytes + 1))).toThrow("1 MiB");
    });

    it.each([
        ["zero-total", [0, 0]],
        ["overflowing", [Number.MAX_VALUE, Number.MAX_VALUE]],
        ["non-fractional", [1, 1]],
    ])("rejects %s split geometry", (_label, sizes) => {
        const root = { type: "split", id: "root", dir: "row", children: [pane("left"), pane("right")], sizes };
        expect(() =>
            parseSessionBundle(JSON.stringify(bundle({ windows: [{ id: "window-1", name: "1", role: "term", root, activePaneId: "left" }] }))),
        ).toThrow("invalid or unsupported");
    });
});
