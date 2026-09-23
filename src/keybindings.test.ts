import { describe, expect, it } from "vitest";
import {
    KEYBINDING_ACTIONS,
    actionForEvent,
    eventToKeybinding,
    findKeybindingConflict,
    getKeybindingAction,
    keybindingHasModifier,
    keybindingLabel,
    normaliseKeybindingOverrides,
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
        const sendBinding = getKeybindingAction("bruno.send").defaultBinding;
        expect(actionForEvent(key("NumpadEnter", { metaKey: sendBinding.startsWith("Meta"), ctrlKey: sendBinding.startsWith("Ctrl") }), {})).toBe(
            "bruno.send",
        );
    });

    it("treats the shifted + as the plain = it shares a key with", () => {
        const increase = getKeybindingAction("terminal.fontIncrease").defaultBinding;
        const held = { metaKey: increase.startsWith("Meta"), ctrlKey: increase.startsWith("Ctrl") };
        expect(actionForEvent(key("Equal", held), {})).toBe("terminal.fontIncrease");
        expect(actionForEvent(key("Equal", { ...held, shiftKey: true }), {})).toBe("terminal.fontIncrease");
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
        expect(findKeybindingConflict(overrides, "aws.open", "Ctrl+Shift+KeyO")?.id).toBe("project.open");
    });

    it("keeps every default binding unique and routes both session actions", () => {
        const owners = new Map<string, string[]>();
        for (const action of KEYBINDING_ACTIONS) {
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
