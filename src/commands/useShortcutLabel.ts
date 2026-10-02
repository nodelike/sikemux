import { getState, useStore } from "../state/store";
import { keybindingLabel, resolvedKeybinding, type KeybindingActionId } from "./keybindings";

function labelOf(binding: string | null): string {
    return binding ? keybindingLabel(binding) : "";
}

/** The key an action is bound to right now, as shown in a hint, or "" when it has none. */
export function useShortcutLabel(id: KeybindingActionId): string {
    return labelOf(useStore((state) => resolvedKeybinding(state.keybindingOverrides, id)));
}

/** The same, read once, for a menu built when it opens. */
export function currentShortcutLabel(id: KeybindingActionId): string {
    return labelOf(resolvedKeybinding(getState().keybindingOverrides, id));
}

/** A hint's text with the action's key after it, when it has one. */
export function withShortcut(text: string, shortcut: string): string {
    return shortcut ? `${text} — ${shortcut}` : text;
}
