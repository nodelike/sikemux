import { IS_MACOS } from "./lib/platform";

export type KeybindingCategory = "Workspace" | "Panes" | "Navigation" | "Bruno";

export interface KeybindingAction {
    id: string;
    label: string;
    detail: string;
    category: KeybindingCategory;
    defaultBinding: string;
}

const keybindingActions = [
    {
        id: "palette.commands",
        label: "Open command deck",
        detail: "Search every action, shortcut, and custom command",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Shift+KeyP`,
    },
    {
        id: "palette.files",
        label: "Open file or request palette",
        detail: "Files in projects, requests in Bruno",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyP`,
    },
    {
        id: "search.global",
        label: "Global search",
        detail: "Search across the active project",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Shift+KeyF`,
    },
    {
        id: "settings.toggle",
        label: "Open settings",
        detail: "Open or close this preferences window",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Comma`,
    },
    {
        id: "session.open",
        label: "Open or create session",
        detail: "Show every available session type",
        category: "Workspace",
        defaultBinding: "Alt+KeyS",
    },
    {
        id: "project.open",
        label: "Open project",
        detail: "Open the project picker",
        category: "Workspace",
        defaultBinding: "Alt+KeyP",
    },
    {
        id: "ssh.open",
        label: "Connect to SSH host",
        detail: "Open the SSH host picker",
        category: "Workspace",
        defaultBinding: "Alt+Shift+KeyS",
    },
    {
        id: "aws.open",
        label: "Open AWS",
        detail: "Create an AWS session",
        category: "Workspace",
        defaultBinding: "Alt+KeyA",
    },
    {
        id: "bruno.open",
        label: "Open Bruno workspace",
        detail: "Choose a Bruno collection",
        category: "Workspace",
        defaultBinding: "Alt+KeyB",
    },
    {
        id: "session.newContextual",
        label: "New item",
        detail: "Create a window or session for the current context",
        category: "Workspace",
        defaultBinding: "Alt+KeyN",
    },
    {
        id: "session.close",
        label: "Close session",
        detail: "Close the active session",
        category: "Workspace",
        defaultBinding: "Alt+KeyQ",
    },
    {
        id: "session.command",
        label: "Focus command session",
        detail: "Jump to the command terminal",
        category: "Workspace",
        defaultBinding: "Alt+KeyT",
    },
    {
        id: "pane.splitRow",
        label: "Split pane right",
        detail: "Create a side-by-side pane",
        category: "Panes",
        defaultBinding: "Alt+Backslash",
    },
    {
        id: "pane.splitColumn",
        label: "Split pane down",
        detail: "Create a stacked pane",
        category: "Panes",
        defaultBinding: "Alt+Minus",
    },
    {
        id: "pane.focusLeft",
        label: "Focus pane left",
        detail: "Move focus to the pane on the left",
        category: "Panes",
        defaultBinding: "Alt+KeyH",
    },
    {
        id: "pane.focusDown",
        label: "Focus pane down",
        detail: "Move focus to the pane below",
        category: "Panes",
        defaultBinding: "Alt+KeyJ",
    },
    {
        id: "pane.focusUp",
        label: "Focus pane up",
        detail: "Move focus to the pane above",
        category: "Panes",
        defaultBinding: "Alt+KeyK",
    },
    {
        id: "pane.focusRight",
        label: "Focus pane right",
        detail: "Move focus to the pane on the right",
        category: "Panes",
        defaultBinding: "Alt+KeyL",
    },
    {
        id: "pane.resizeLeft",
        label: "Resize pane left",
        detail: "Grow the active pane toward the left",
        category: "Panes",
        defaultBinding: "Alt+Shift+KeyH",
    },
    {
        id: "pane.resizeDown",
        label: "Resize pane down",
        detail: "Grow the active pane downward",
        category: "Panes",
        defaultBinding: "Alt+Shift+KeyJ",
    },
    {
        id: "pane.resizeUp",
        label: "Resize pane up",
        detail: "Grow the active pane upward",
        category: "Panes",
        defaultBinding: "Alt+Shift+KeyK",
    },
    {
        id: "pane.resizeRight",
        label: "Resize pane right",
        detail: "Grow the active pane toward the right",
        category: "Panes",
        defaultBinding: "Alt+Shift+KeyL",
    },
    {
        id: "pane.zoom",
        label: "Zoom pane",
        detail: "Toggle focus mode for the active pane",
        category: "Panes",
        defaultBinding: "Alt+KeyZ",
    },
    {
        id: "pane.close",
        label: "Close focused pane",
        detail: "Close the current pane or focused agent",
        category: "Panes",
        defaultBinding: "Alt+KeyW",
    },
    {
        id: "window.previous",
        label: "Previous window",
        detail: "Move to the previous workspace window",
        category: "Navigation",
        defaultBinding: "Alt+BracketLeft",
    },
    {
        id: "window.next",
        label: "Next window",
        detail: "Move to the next workspace window",
        category: "Navigation",
        defaultBinding: "Alt+BracketRight",
    },
    {
        id: "tab.previous",
        label: "Previous terminal tab",
        detail: "Move to the previous terminal tab",
        category: "Navigation",
        defaultBinding: "Alt+Comma",
    },
    {
        id: "tab.next",
        label: "Next terminal tab",
        detail: "Move to the next terminal tab",
        category: "Navigation",
        defaultBinding: "Alt+Period",
    },
    {
        id: "session.lastUsed",
        label: "Switch to last-used session",
        detail: "Toggle back to the session you used immediately before this one",
        category: "Navigation",
        defaultBinding: "Alt+KeyQ",
    },
    {
        id: "session.next",
        label: "Next session",
        detail: "Cycle forward through sessions",
        category: "Navigation",
        defaultBinding: "Alt+Tab",
    },
    {
        id: "session.previous",
        label: "Previous session",
        detail: "Cycle backward through sessions",
        category: "Navigation",
        defaultBinding: "Alt+Backquote",
    },
    {
        id: "session.nextGroup",
        label: "Next session group",
        detail: "Cycle through project, SSH, cloud and command groups",
        category: "Navigation",
        defaultBinding: "Alt+Shift+Tab",
    },
    {
        id: "window.files",
        label: "Focus files",
        detail: "Jump to the files window",
        category: "Navigation",
        defaultBinding: "Alt+Digit1",
    },
    {
        id: "window.terminal",
        label: "Focus terminal",
        detail: "Jump to the terminal window",
        category: "Navigation",
        defaultBinding: "Alt+Digit2",
    },
    {
        id: "window.git",
        label: "Focus Git",
        detail: "Jump to the Git window",
        category: "Navigation",
        defaultBinding: "Alt+Digit3",
    },
    {
        id: "window.agents",
        label: "Focus agents",
        detail: "Jump to the agents view",
        category: "Navigation",
        defaultBinding: "Alt+Digit4",
    },
    {
        id: "window.search",
        label: "Focus search",
        detail: "Jump to the search window",
        category: "Navigation",
        defaultBinding: "Alt+Digit5",
    },
    {
        id: "agent.permissions",
        label: "Toggle agent permissions",
        detail: "Toggle skip-permissions for the active agent",
        category: "Navigation",
        defaultBinding: "Alt+KeyY",
    },
    {
        id: "bruno.save",
        label: "Save request",
        detail: "Save the active Bruno request",
        category: "Bruno",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyS`,
    },
    {
        id: "bruno.send",
        label: "Send request",
        detail: "Run the active Bruno request",
        category: "Bruno",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Enter`,
    },
    {
        id: "bruno.environment",
        label: "Choose environment",
        detail: "Open the Bruno environment picker",
        category: "Bruno",
        defaultBinding: "Alt+KeyE",
    },
] as const satisfies readonly KeybindingAction[];

export type KeybindingActionId = (typeof keybindingActions)[number]["id"];
export type KeybindingOverrides = Partial<Record<KeybindingActionId, string | null>>;

export const KEYBINDING_ACTIONS: readonly KeybindingAction[] = keybindingActions;
export const KEYBINDING_CATEGORIES: readonly KeybindingCategory[] = ["Workspace", "Panes", "Navigation", "Bruno"];

const ACTION_IDS = new Set<string>(KEYBINDING_ACTIONS.map((action) => action.id));
const ACTIONS_BY_ID = new Map<string, KeybindingAction>(KEYBINDING_ACTIONS.map((action) => [action.id, action]));
const MODIFIER_CODES = new Set(["MetaLeft", "MetaRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight"]);

export function getKeybindingAction(id: KeybindingActionId): KeybindingAction {
    return ACTIONS_BY_ID.get(id) as KeybindingAction;
}

export function resolvedKeybinding(overrides: KeybindingOverrides, id: KeybindingActionId): string | null {
    const override = overrides[id];
    return override === undefined ? getKeybindingAction(id).defaultBinding : override;
}

export function eventToKeybinding(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">): string | null {
    if (!event.code || MODIFIER_CODES.has(event.code)) return null;
    const parts: string[] = [];
    if (event.metaKey) parts.push("Meta");
    if (event.ctrlKey) parts.push("Ctrl");
    if (event.altKey) parts.push("Alt");
    if (event.shiftKey) parts.push("Shift");
    parts.push(event.code);
    return parts.join("+");
}

export function keybindingHasModifier(binding: string): boolean {
    const parts = binding.split("+");
    return parts.includes("Meta") || parts.includes("Ctrl") || parts.includes("Alt") || parts.includes("Shift");
}

export function matchesKeybinding(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, binding: string): boolean {
    const eventBinding = eventToKeybinding(event);
    if (eventBinding === binding) return true;
    // The main and numpad Enter keys are interchangeable for command shortcuts.
    return event.code === "NumpadEnter" && binding.endsWith("+Enter") && eventBinding === binding.replace(/\+Enter$/, "+NumpadEnter");
}

export function actionForEvent(
    event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
    overrides: KeybindingOverrides,
): KeybindingActionId | null {
    for (const action of KEYBINDING_ACTIONS) {
        const binding = resolvedKeybinding(overrides, action.id as KeybindingActionId);
        if (binding && matchesKeybinding(event, binding)) return action.id as KeybindingActionId;
    }
    return null;
}

export function findKeybindingConflict(overrides: KeybindingOverrides, id: KeybindingActionId, binding: string): KeybindingAction | null {
    return (
        KEYBINDING_ACTIONS.find((action) => action.id !== id && resolvedKeybinding(overrides, action.id as KeybindingActionId) === binding) ?? null
    );
}

const CODE_LABELS: Record<string, string> = {
    Backquote: "`",
    Backslash: "\\",
    BracketLeft: "[",
    BracketRight: "]",
    Comma: ",",
    Enter: "↵",
    Equal: "=",
    Escape: "Esc",
    Minus: "-",
    NumpadEnter: "Num ↵",
    Period: ".",
    Quote: "'",
    Semicolon: ";",
    Slash: "/",
    Space: "Space",
    Tab: "Tab",
};

function codeLabel(code: string): string {
    if (CODE_LABELS[code]) return CODE_LABELS[code];
    if (/^Key[A-Z]$/.test(code)) return code.slice(3);
    if (/^Digit[0-9]$/.test(code)) return code.slice(5);
    if (/^Numpad[0-9]$/.test(code)) return `Num ${code.slice(6)}`;
    if (/^Arrow/.test(code)) return code.slice(5);
    return code.replace(/([a-z])([A-Z])/g, "$1 $2");
}

export function keybindingLabel(binding: string | null): string {
    if (!binding) return "Unassigned";
    const parts = binding.split("+");
    const code = parts.pop() ?? "";
    const modifiers = parts
        .map((part) => {
            if (part === "Meta") return IS_MACOS ? "⌘" : "Meta+";
            if (part === "Ctrl") return IS_MACOS ? "⌃" : "Ctrl+";
            if (part === "Alt") return IS_MACOS ? "⌥" : "Alt+";
            if (part === "Shift") return IS_MACOS ? "⇧" : "Shift+";
            return part;
        })
        .join("");
    return `${modifiers}${codeLabel(code)}`;
}

export function keybindingLabelForAction(overrides: KeybindingOverrides, id: KeybindingActionId): string {
    return keybindingLabel(resolvedKeybinding(overrides, id));
}

export function normaliseKeybindingOverrides(value: unknown): KeybindingOverrides {
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const out: KeybindingOverrides = {};
    for (const [id, binding] of Object.entries(value)) {
        if (!ACTION_IDS.has(id)) continue;
        if (binding === null) {
            out[id as KeybindingActionId] = null;
            continue;
        }
        if (typeof binding !== "string") continue;
        const pieces = binding.split("+");
        const code = pieces.at(-1);
        if (!code || MODIFIER_CODES.has(code) || !keybindingHasModifier(binding)) continue;
        out[id as KeybindingActionId] = binding;
    }
    return out;
}
