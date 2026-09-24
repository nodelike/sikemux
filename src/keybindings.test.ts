import { describe, expect, it, vi } from "vitest";
import {
    keybindingActions,
    actionForEvent,
    eventToKeybinding,
    findKeybindingConflict,
    getKeybindingAction,
    keybindingHasModifier,
    keybindingLabel,
    normaliseKeybindingOverrides,
    pluginOpenedBy,
    pluginShortcutFor,
    keybindingCategories,
    resolvedKeybinding,
} from "./keybindings";

function key(code: string, modifiers: Partial<Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "altKey" | "shiftKey">> = {}) {
    return {
        code,
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
        ...modifiers,
    };
}

describe("keybindings", () => {
    it("serializes physical keys and renders platform labels", () => {
        const binding = eventToKeybinding(key("KeyF", { ctrlKey: true, shiftKey: true }));
        expect(binding).toBe("Ctrl+Shift+KeyF");
        expect(keybindingLabel(binding)).toMatch(/F$/);
        expect(keybindingLabel("Alt+Backslash")).toMatch(/\\$/);
    });

    it("answers a shortcut on Enter from the numpad Enter too", async () => {
        await import("./plugins/builtin");
        const send = getKeybindingAction("plugin.run:sikemux.bruno/send")?.defaultBinding ?? "";
        expect(actionForEvent(key("NumpadEnter", { metaKey: send.startsWith("Meta"), ctrlKey: send.startsWith("Ctrl") }), {})).toBe(
            "plugin.run:sikemux.bruno/send",
        );
    });

    it("treats the shifted + as the plain = it shares a key with", () => {
        const increase = getKeybindingAction("text.sizeIncrease").defaultBinding;
        const held = { metaKey: increase.startsWith("Meta"), ctrlKey: increase.startsWith("Ctrl") };
        expect(actionForEvent(key("Equal", held), {})).toBe("text.sizeIncrease");
        expect(actionForEvent(key("Equal", { ...held, shiftKey: true }), {})).toBe("text.sizeIncrease");
    });

    it("resolves defaults, replacements, and explicit unassignment", () => {
        expect(resolvedKeybinding({}, "settings.toggle")).toBe(getKeybindingAction("settings.toggle").defaultBinding);
        expect(resolvedKeybinding({ "settings.toggle": "Ctrl+Comma" }, "settings.toggle")).toBe("Ctrl+Comma");
        expect(resolvedKeybinding({ "settings.toggle": null }, "settings.toggle")).toBeNull();
    });

    it("routes an event through overrides and reports conflicts", () => {
        const overrides = { "project.open": "Ctrl+Shift+KeyO" } as const;
        expect(actionForEvent(key("KeyO", { ctrlKey: true, shiftKey: true }), overrides)).toBe("project.open");
        expect(actionForEvent(key("KeyP", { altKey: true }), overrides)).toBeNull();
        expect(findKeybindingConflict(overrides, "ssh.open", "Ctrl+Shift+KeyO")?.id).toBe("project.open");
    });

    it("keeps every default binding unique and routes both session actions", () => {
        const owners = new Map<string, string[]>();
        for (const action of keybindingActions()) {
            const bindingOwners = owners.get(action.defaultBinding) ?? [];
            bindingOwners.push(action.id);
            owners.set(action.defaultBinding, bindingOwners);
        }

        expect(Array.from(owners, ([binding, ids]) => ({ binding, ids })).filter(({ ids }) => ids.length > 1)).toEqual([]);
        expect(actionForEvent(key("KeyQ", { altKey: true }), {})).toBe("session.close");
        expect(actionForEvent(key("KeyU", { altKey: true }), {})).toBe("session.lastUsed");
    });

    it("requires a modifier for user-recorded shortcuts", () => {
        expect(keybindingHasModifier("KeyA")).toBe(false);
        expect(keybindingHasModifier("Shift+KeyA")).toBe(true);
    });

    it("sanitizes persisted overrides", () => {
        expect(
            normaliseKeybindingOverrides({
                "project.open": "Ctrl+KeyP",
                "pane.zoom": null,
                "unknown.action": "Meta+KeyU",
                "aws.open": "KeyA",
                "session.open": 42,
            }),
        ).toEqual({
            "project.open": "Ctrl+KeyP",
            "pane.zoom": null,
        });
    });
});

describe("plugin shortcuts", () => {
    it("lists a plugin's open shortcut once it registers, and knows which plugin it opens", async () => {
        await import("./plugins/builtin");
        const aws = keybindingActions().find((action) => action.id === "plugin.open:sikemux.aws");
        expect(aws).toMatchObject({ label: "Open AWS", defaultBinding: "Alt+KeyA" });
        expect(pluginOpenedBy("plugin.open:sikemux.aws")).toBe("sikemux.aws");
        expect(pluginOpenedBy("ssh.open")).toBeNull();
        expect(normaliseKeybindingOverrides({ "plugin.open:sikemux.aws": "Alt+Shift+KeyA", "aws.open": "Alt+KeyZ" })).toEqual({
            "plugin.open:sikemux.aws": "Alt+Shift+KeyA",
        });
    });
});

describe("a plugin's own shortcuts", () => {
    it("are listed under the plugin's name and run only when they apply", async () => {
        const { registerFrontendPlugin } = await import("./plugins/registry");
        let applies = true;
        const run = vi.fn(() => applies);
        registerFrontendPlugin({
            id: "test.shortcuts",
            surfaces: [{ kind: "test.shortcuts:main", title: "Tester", icon: () => null, render: () => null }],
            open: () => {},
            openTitle: "Open Tester",
            shortcuts: [{ name: "go", label: "Go", detail: "Run the test", defaultBinding: "Alt+Shift+KeyG", run }],
        });

        expect(keybindingCategories()).toContain("Tester");
        expect(keybindingActions().find((action) => action.id === "plugin.run:test.shortcuts/go")).toMatchObject({
            label: "Go",
            category: "Tester",
            defaultBinding: "Alt+Shift+KeyG",
        });
        expect(actionForEvent(key("KeyG", { altKey: true, shiftKey: true }), {})).toBe("plugin.run:test.shortcuts/go");
        expect(pluginShortcutFor("plugin.run:test.shortcuts/go")?.run()).toBe(true);
        applies = false;
        expect(pluginShortcutFor("plugin.run:test.shortcuts/go")?.run()).toBe(false);
        expect(pluginShortcutFor("plugin.run:test.shortcuts/missing")).toBeNull();
    });
});
