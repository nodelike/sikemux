import { describe, expect, it } from "vitest";
import { DESK_PERSISTENCE_LIMITS, EDITOR_PERSISTENCE_LIMITS, decodeItemState, encodeItemState } from "./itemStates";

const EDITOR = { id: "pane-editor", kind: "editor" } as const;
const DESK = { id: "pane-desk", kind: "desk" } as const;

const editorEnvelope = (state: unknown) => ({ itemId: "pane-editor", kind: "editor", version: 2, state });
const deskEnvelope = (state: unknown) => ({ itemId: "pane-desk", kind: "desk", version: 1, state });

describe("saved editor state", () => {
    it("round-trips with exact ID, kind and version matching", () => {
        const state = { openTabs: ["/project/a.ts", "/project/b.ts"], activePath: "/project/b.ts" };
        const encoded = encodeItemState("pane-editor", "editor", state);

        expect(encoded).toEqual({ itemId: "pane-editor", kind: "editor", version: 2, state });
        expect(decodeItemState(EDITOR, encoded)).toEqual(state);
        expect(decodeItemState({ id: "pane-other", kind: "editor" }, encoded)).toBeNull();
        expect(decodeItemState(EDITOR, { ...encoded, version: 1 })).toBeNull();
        expect(decodeItemState(EDITOR, { ...encoded, kind: "desk" })).toBeNull();
    });

    it("preserves valid Windows paths, spaces, limit-sized paths, and nullable selection", () => {
        const longestPath = `/${"a".repeat(EDITOR_PERSISTENCE_LIMITS.maxPathLength - 1)}`;
        const state = { openTabs: ["C:\\Project Files\\hello world.ts", "/project/file with spaces.ts", longestPath], activePath: null };

        expect(decodeItemState(EDITOR, editorEnvelope(state))).toEqual(state);
    });

    it("keeps a view split beside other work as showing one file, and nothing else in that field", () => {
        const state = { openTabs: ["/project/a.ts"], activePath: "/project/a.ts", single: true };

        expect(decodeItemState(EDITOR, editorEnvelope(state))).toEqual(state);
        expect(decodeItemState(EDITOR, editorEnvelope({ ...state, single: false }))).toBeNull();
    });

    it("keeps the preview tab, and drops one that is no longer open", () => {
        const state = { openTabs: ["/project/a.ts", "/project/b.ts"], activePath: "/project/b.ts", preview: "/project/b.ts" };

        expect(decodeItemState(EDITOR, editorEnvelope(state))).toEqual(state);
        expect(decodeItemState(EDITOR, editorEnvelope({ ...state, preview: "/project/gone.ts" }))).toEqual({
            openTabs: state.openTabs,
            activePath: state.activePath,
        });
    });

    it("accepts the tab-count boundary and rejects one tab beyond it", () => {
        const openTabs = Array.from({ length: EDITOR_PERSISTENCE_LIMITS.maxOpenTabs }, (_value, index) => `/project/${index}.ts`);
        const state = { openTabs, activePath: openTabs.at(-1) ?? null };

        expect(decodeItemState(EDITOR, editorEnvelope(state))).toEqual(state);
        expect(
            decodeItemState(EDITOR, editorEnvelope({ openTabs: [...openTabs, "/project/overflow.ts"], activePath: "/project/overflow.ts" })),
        ).toBeNull();
    });

    it.each([
        ["empty path", [""], null],
        ["blank path", ["   "], null],
        ["oversized path", [`/${"a".repeat(EDITOR_PERSISTENCE_LIMITS.maxPathLength)}`], null],
        ["duplicate path", ["/project/a.ts", "/project/a.ts"], "/project/a.ts"],
        ["non-string path", ["/project/a.ts", 7], "/project/a.ts"],
        ["active path is not open", ["/project/a.ts"], "/project/missing.ts"],
        ["empty active path", ["/project/a.ts"], ""],
    ])("rejects %s", (_label, openTabs, activePath) => {
        expect(decodeItemState(EDITOR, editorEnvelope({ openTabs, activePath }))).toBeNull();
    });
});

describe("saved desk state", () => {
    it("round-trips the pages and files a desk can open again", () => {
        const state = { agentId: "agent-one", tabs: [{ url: "https://example.com/docs", title: "Docs" }], activeIndex: 0, files: ["/repo/a.ts"] };
        const filesOnly = { agentId: "agent-one", tabs: [], activeIndex: 0, files: ["/repo/a.ts"] };

        expect(decodeItemState(DESK, encodeItemState("pane-desk", "desk", state))).toEqual(state);
        expect(decodeItemState(DESK, encodeItemState("pane-desk", "desk", filesOnly))).toEqual(filesOnly);
    });

    /* These come back off disk and a restored tab loads itself, so a scheme
       that can reach the machine or run on its own must not survive the trip. */
    it.each([
        ["a scheme that is not the web", { url: "file:///etc/passwd", title: "" }],
        ["a script url", { url: "javascript:alert(1)", title: "" }],
        ["an inline document", { url: "data:text/html,<b>hi</b>", title: "" }],
        ["a url that is only a scheme", { url: "https://", title: "" }],
    ])("refuses %s in a saved desk page", (_label, tab) => {
        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: [tab], activeIndex: 0, files: [] }))).toBeNull();
    });

    it("refuses a saved desk with nothing on it, too much, an active page that is not there, or a file twice", () => {
        const page = { url: "https://example.com", title: "Example" };
        const tooMany = Array.from({ length: DESK_PERSISTENCE_LIMITS.maxTabs + 1 }, () => page);

        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: [], activeIndex: 0, files: [] }))).toBeNull();
        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: tooMany, activeIndex: 0, files: [] }))).toBeNull();
        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: [page], activeIndex: 1, files: [] }))).toBeNull();
        expect(decodeItemState(DESK, deskEnvelope({ agentId: " ", tabs: [page], activeIndex: 0, files: [] }))).toBeNull();
        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: [], activeIndex: 0, files: ["/a", "/a"] }))).toBeNull();
        expect(decodeItemState(DESK, deskEnvelope({ agentId: "agent-one", tabs: [page], activeIndex: 0 }))).toBeNull();
    });
});
