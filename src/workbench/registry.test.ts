import { describe, expect, it } from "vitest";
import type { CorePaneKind, PaneKind, PaneNode } from "../state/types/domain";
import {
    BROWSER_PERSISTENCE_LIMITS,
    BUILTIN_WORKBENCH_ITEM_MANIFEST,
    DuplicateWorkbenchItemKindError,
    EDITOR_PERSISTENCE_LIMITS,
    UnknownWorkbenchItemKindError,
    WorkbenchItemRegistry,
    createItemId,
    createWorkbenchItemRef,
    workbenchItemRefFromPane,
    type PersistedCodecResult,
    type WorkbenchItemController,
    type WorkbenchItemDefinition,
} from "./registry";

const BUILTIN_KINDS = ["terminal", "editor", "git", "diff", "search", "agent", "browser"] as const satisfies readonly CorePaneKind[];

function nullEnvelope(itemId: string, kind: PaneKind, overrides: Record<string, unknown> = {}): Record<string, unknown> {
    return { itemId, kind, version: 1, state: null, ...overrides };
}

function editorEnvelope(state: unknown): Record<string, unknown> {
    return { itemId: "pane-editor", kind: "editor", version: 2, state };
}

describe("built-in workbench item manifest", () => {
    it("is exhaustive for current CorePaneKind values and creates safe no-op controllers", async () => {
        expect(Object.keys(BUILTIN_WORKBENCH_ITEM_MANIFEST)).toEqual(BUILTIN_KINDS);
        const registry = new WorkbenchItemRegistry();

        for (const kind of BUILTIN_KINDS) {
            const definition = BUILTIN_WORKBENCH_ITEM_MANIFEST[kind];
            expect(definition.kind).toBe(kind);
            expect(definition.defaultTitle).not.toBe("");
            expect(Number.isInteger(definition.persisted.version)).toBe(true);
            expect(definition.persisted.version).toBeGreaterThan(0);

            const controller = registry.create(createWorkbenchItemRef(`pane-${kind}`, kind));
            await controller.activate();
            await controller.deactivate();
            expect(await controller.canClose()).toBe(true);
            await controller.dispose();
        }
        expect(registry.kinds()).toEqual(BUILTIN_KINDS);
    });

    it("derives a typed item reference from the existing PaneNode", () => {
        const pane: PaneNode = { type: "pane", id: "pane-editor", cwd: "/project", kind: "editor", title: "Editor" };
        const ref = workbenchItemRefFromPane(pane);

        expect(ref).toEqual({ id: "pane-editor", kind: "editor" });
        expect(createItemId("pane-1")).toBe("pane-1");
        expect(() => createItemId(" pane-1 ")).toThrow(TypeError);
        expect(() => createItemId("pane\n1")).toThrow(TypeError);
        expect(() => createItemId("x".repeat(257))).toThrow(TypeError);
        expect(() => createItemId("__proto__")).toThrow(TypeError);
        expect(() => createItemId("constructor")).toThrow(TypeError);
    });
});

describe("WorkbenchItemRegistry lifecycle", () => {
    it("runs custom controller lifecycle and cleanup hooks", async () => {
        const events: string[] = [];
        const controller: WorkbenchItemController = {
            activate: () => {
                events.push("activate");
            },
            deactivate: async () => {
                events.push("deactivate");
            },
            canClose: () => {
                events.push("can-close");
                return false;
            },
            dispose: () => {
                events.push("dispose");
            },
        };
        const definition: WorkbenchItemDefinition<"notes", string> = {
            kind: "notes",
            defaultTitle: "Notes",
            create: (ref) => {
                events.push(`create:${ref.id}`);
                return controller;
            },
            persisted: {
                version: 3,
                encode: (state) => state,
                decode: (encoded): PersistedCodecResult<string> => (typeof encoded === "string" ? { ok: true, value: encoded } : { ok: false }),
            },
            cleanupDraft: async (state) => {
                events.push(`cleanup:${state}`);
            },
        };
        const registry = new WorkbenchItemRegistry();
        registry.register(definition);
        const ref = createWorkbenchItemRef("item-notes", "notes");
        const created = registry.create(ref);

        await created.activate();
        await created.deactivate();
        expect(await created.canClose()).toBe(false);
        await registry.cleanupDraft(ref, "draft");
        await created.dispose();

        expect(events).toEqual(["create:item-notes", "activate", "deactivate", "can-close", "cleanup:draft", "dispose"]);
    });

    it("rejects duplicate and unknown registrations", () => {
        const registry = new WorkbenchItemRegistry();
        expect(() => registry.register(BUILTIN_WORKBENCH_ITEM_MANIFEST.editor)).toThrow(DuplicateWorkbenchItemKindError);
        expect(() => registry.get("missing")).toThrow(UnknownWorkbenchItemKindError);
        expect(() => registry.create(createWorkbenchItemRef("item-missing", "missing"))).toThrow(UnknownWorkbenchItemKindError);
    });
});

describe("workbench item persistence", () => {
    it("round-trips isolated editor state with exact ID, kind, and version matching", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");
        const openTabs = ["/project/a.ts", "/project/b.ts"];
        const encoded = registry.encodePersisted(ref, { openTabs, activePath: "/project/b.ts" });
        openTabs.push("/project/not-persisted.ts");

        expect(encoded).toEqual({
            itemId: "pane-editor",
            kind: "editor",
            version: 2,
            state: {
                openTabs: ["/project/a.ts", "/project/b.ts"],
                activePath: "/project/b.ts",
            },
        });
        const decoded = registry.decodePersisted(ref, encoded);
        expect(decoded).toEqual({
            ok: true,
            ref,
            state: {
                openTabs: ["/project/a.ts", "/project/b.ts"],
                activePath: "/project/b.ts",
            },
        });
        if (decoded.ok) expect(decoded.state).not.toBe(encoded.state);
    });

    it("preserves valid Windows paths, spaces, limit-sized paths, and nullable selection", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");
        const longestPath = `/${"a".repeat(EDITOR_PERSISTENCE_LIMITS.maxPathLength - 1)}`;
        const state = {
            openTabs: ["C:\\Project Files\\hello world.ts", "/project/file with spaces.ts", longestPath],
            activePath: null,
        };

        expect(registry.decodePersisted(ref, editorEnvelope(state))).toEqual({ ok: true, ref, state });
    });

    it("accepts the tab-count boundary and rejects one tab beyond it", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");
        const openTabs = Array.from({ length: EDITOR_PERSISTENCE_LIMITS.maxOpenTabs }, (_value, index) => `/project/${index}.ts`);
        const state = { openTabs, activePath: openTabs.at(-1) ?? null };

        expect(registry.decodePersisted(ref, editorEnvelope(state))).toMatchObject({ ok: true, state });
        expect(
            registry.decodePersisted(
                ref,
                editorEnvelope({ ...state, openTabs: [...openTabs, "/project/overflow.ts"], activePath: "/project/overflow.ts" }),
            ),
        ).toEqual({ ok: false, reason: "invalid-state" });
    });

    it.each([
        ["empty path", [""], null],
        ["blank path", ["   "], null],
        ["oversized path", [`/${"a".repeat(EDITOR_PERSISTENCE_LIMITS.maxPathLength)}`], null],
        ["C0 control character", [`/project/bad${String.fromCharCode(10)}path.ts`], null],
        ["C1 control character", [`/project/bad${String.fromCharCode(159)}path.ts`], null],
        ["duplicate path", ["/project/a.ts", "/project/a.ts"], "/project/a.ts"],
        ["non-string path", ["/project/a.ts", 7], "/project/a.ts"],
        ["active path is not open", ["/project/a.ts"], "/project/missing.ts"],
        ["empty active path", ["/project/a.ts"], ""],
    ])("rejects bounded editor path violation: %s", (_label, openTabs, activePath) => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");
        expect(registry.decodePersisted(ref, editorEnvelope({ openTabs, activePath }))).toEqual({
            ok: false,
            reason: "invalid-state",
        });
    });

    it("rejects duplicate open paths while encoding live editor state", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");

        expect(registry.encodePersisted(ref, { openTabs: [], activePath: null }).state).toEqual({ openTabs: [], activePath: null });
        expect(() => registry.encodePersisted(ref, { openTabs: ["/project/a.ts", "/project/a.ts"], activePath: "/project/a.ts" })).toThrow(TypeError);
    });

    it("round-trips null state for every kind that keeps no state of its own", () => {
        const registry = new WorkbenchItemRegistry();
        for (const kind of BUILTIN_KINDS.filter((candidate) => candidate !== "editor" && candidate !== "browser")) {
            const ref = createWorkbenchItemRef(`pane-${kind}`, kind);
            const encoded = registry.encodePersisted(ref, null);
            expect(registry.decodePersisted(ref, encoded)).toEqual({ ok: true, ref, state: null });
        }
    });

    it("round-trips the tabs a browser pane can open again", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-browser", "browser");
        const state = { agentId: "agent-one", tabs: [{ url: "https://example.com/docs", title: "Docs" }], activeIndex: 0 };

        expect(registry.decodePersisted(ref, registry.encodePersisted(ref, state))).toEqual({ ok: true, ref, state });
    });

    /* These come back off disk and a restored tab loads itself, so a scheme
       that can reach the machine or run on its own must not survive the trip. */
    it.each([
        ["a scheme that is not the web", { url: "file:///etc/passwd", title: "" }],
        ["a script url", { url: "javascript:alert(1)", title: "" }],
        ["an inline document", { url: "data:text/html,<b>hi</b>", title: "" }],
        ["a url that is only a scheme", { url: "https://", title: "" }],
        ["a title carrying control characters", { url: "https://example.com", title: "one\u0000two" }],
    ])("refuses %s in a saved browser tab", (_label, tab) => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-browser", "browser");

        expect(
            registry.decodePersisted(ref, {
                itemId: "pane-browser",
                kind: "browser",
                version: 1,
                state: { agentId: "agent-one", tabs: [tab], activeIndex: 0 },
            }),
        ).toEqual({
            ok: false,
            reason: "invalid-state",
        });
    });

    it("refuses a saved browser pane with no tabs, too many, or an active tab that is not there", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-browser", "browser");
        const page = { url: "https://example.com", title: "Example" };
        const envelope = (state: unknown) => ({ itemId: "pane-browser", kind: "browser", version: 1, state });

        expect(registry.decodePersisted(ref, envelope({ agentId: "agent-one", tabs: [], activeIndex: 0 })).ok).toBe(false);
        expect(
            registry.decodePersisted(
                ref,
                envelope({ agentId: "agent-one", tabs: Array.from({ length: BROWSER_PERSISTENCE_LIMITS.maxTabs + 1 }, () => page), activeIndex: 0 }),
            ).ok,
        ).toBe(false);
        expect(registry.decodePersisted(ref, envelope({ agentId: "agent-one", tabs: [page], activeIndex: 1 })).ok).toBe(false);
        expect(registry.decodePersisted(ref, envelope({ agentId: " ", tabs: [page], activeIndex: 0 })).ok).toBe(false);
    });

    it.each([
        ["non-object", null, "invalid-envelope"],
        ["missing state", { itemId: "pane-terminal", kind: "terminal", version: 1 }, "invalid-envelope"],
        ["extra field", { ...nullEnvelope("pane-terminal", "terminal"), payload: "nope" }, "invalid-envelope"],
        ["accessor", Object.defineProperty({}, "itemId", { enumerable: true, get: () => "pane-terminal" }), "invalid-envelope"],
        ["invalid item ID", nullEnvelope(" bad ", "terminal"), "invalid-envelope"],
        ["different item ID", nullEnvelope("pane-other", "terminal"), "item-id-mismatch"],
        ["different kind", nullEnvelope("pane-terminal", "git"), "kind-mismatch"],
        ["unknown kind", nullEnvelope("pane-terminal", "terminal", { kind: "notes" }), "unknown-kind"],
        ["different version", nullEnvelope("pane-terminal", "terminal", { version: 2 }), "version-mismatch"],
        ["non-integer version", nullEnvelope("pane-terminal", "terminal", { version: 1.5 }), "invalid-envelope"],
        ["invalid null state", nullEnvelope("pane-terminal", "terminal", { state: {} }), "invalid-state"],
    ])("rejects malformed envelope: %s", (_label, encoded, reason) => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-terminal", "terminal");
        expect(registry.decodePersisted(ref, encoded)).toEqual({ ok: false, reason });
    });

    it("rejects malformed editor view state", () => {
        const registry = new WorkbenchItemRegistry();
        const ref = createWorkbenchItemRef("pane-editor", "editor");
        const malformed = {
            itemId: "pane-editor",
            kind: "editor",
            version: 2,
            state: { openTabs: ["/ok", 7], activePath: "/ok" },
        };

        expect(registry.decodePersisted(ref, malformed)).toEqual({ ok: false, reason: "invalid-state" });
    });

    it("does not let runtime registration widen persisted built-in kinds", () => {
        const registry = new WorkbenchItemRegistry();
        registry.register({
            kind: "notes",
            defaultTitle: "Notes",
            create: () => ({ activate() {}, deactivate() {}, canClose: () => true, dispose() {} }),
            persisted: {
                version: 1,
                encode: (state: string) => state,
                decode: (encoded: unknown): PersistedCodecResult<string> =>
                    typeof encoded === "string" ? { ok: true, value: encoded } : { ok: false },
            },
        });
        const runtimeRef = createWorkbenchItemRef("item-notes", "notes");
        expect(registry.create(runtimeRef).canClose()).toBe(true);

        const forgedBuiltinRef = runtimeRef as unknown as ReturnType<typeof createWorkbenchItemRef<CorePaneKind>>;
        const encoded = { itemId: "item-notes", kind: "notes", version: 1, state: "private draft" };
        expect(registry.decodePersisted(forgedBuiltinRef, encoded)).toEqual({ ok: false, reason: "unknown-kind" });
        expect(() => registry.encodePersisted(forgedBuiltinRef, null)).toThrow(UnknownWorkbenchItemKindError);
    });
});
