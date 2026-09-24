import { IS_MACOS } from "./lib/platform";
import { enabledFrontendPlugins } from "./plugins/enabled";
import { frontendPlugin, frontendPlugins, type FrontendPlugin, type PluginShortcut } from "./plugins/registry";

type CoreKeybindingCategory = "Workspace" | "Panes" | "Navigation" | "Browser";
/** A plugin's own shortcuts are grouped under its name. */
export type KeybindingCategory = CoreKeybindingCategory | (string & {});

export interface KeybindingAction {
    id: string;
    label: string;
    detail: string;
    category: KeybindingCategory;
    defaultBinding: string;
}

const coreKeybindingActions = [
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
        detail: "Files in projects, or what the plugin in front offers",
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
        detail: "Create a pane below",
        category: "Panes",
        defaultBinding: "Alt+Minus",
    },
    {
        id: "pane.splitStack",
        label: "Split pane into tabs",
        detail: "Create a pane in the same place, reached by tab",
        category: "Panes",
        defaultBinding: "Alt+Equal",
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
        id: "text.sizeIncrease",
        label: "Increase text size",
        detail: "Larger text in the focused editor or chat, or in every terminal",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Equal`,
    },
    {
        id: "text.sizeDecrease",
        label: "Decrease text size",
        detail: "Smaller text in the focused editor or chat, or in every terminal",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Minus`,
    },
    {
        id: "text.sizeReset",
        label: "Reset text size",
        detail: "Return the focused editor or chat, or every terminal, to its default size",
        category: "Workspace",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Digit0`,
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
        defaultBinding: "Alt+KeyU",
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
        id: "palette.newTab",
        label: "New tab",
        detail: "Choose what to open: terminal, agent, browser, editor, Git or search",
        category: "Navigation",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyT`,
    },
    {
        id: "browser.tabNew",
        label: "New browser tab",
        detail: "Open the embedded browser for the active agent",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Shift+KeyT`,
    },
    {
        id: "browser.tabClose",
        label: "Close browser tab",
        detail: "Close the active embedded browser tab",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyW`,
    },
    {
        id: "browser.address",
        label: "Focus browser address",
        detail: "Focus the embedded browser address bar",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyL`,
    },
    {
        id: "browser.reload",
        label: "Reload browser tab",
        detail: "Reload the active embedded browser tab",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyR`,
    },
    {
        id: "browser.back",
        label: "Browser back",
        detail: "Go back in the active embedded browser tab",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+BracketLeft`,
    },
    {
        id: "browser.forward",
        label: "Browser forward",
        detail: "Go forward in the active embedded browser tab",
        category: "Browser",
        defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+BracketRight`,
    },
    {
        id: "browser.tabNext",
        label: "Next browser tab",
        detail: "Select the next embedded browser tab",
        category: "Browser",
        defaultBinding: "Ctrl+Tab",
    },
    {
        id: "browser.tabPrevious",
        label: "Previous browser tab",
        detail: "Select the previous embedded browser tab",
        category: "Browser",
        defaultBinding: "Ctrl+Shift+Tab",
    },
] as const satisfies readonly KeybindingAction[];

export type CoreKeybindingActionId = (typeof coreKeybindingActions)[number]["id"];
/** Opens a plugin, for plugins that ask for a shortcut. */
export type PluginOpenActionId = `plugin.open:${string}`;
/** One of a plugin's own shortcuts, as `plugin.run:<plugin id>/<name>`. */
export type PluginRunActionId = `plugin.run:${string}`;
export type KeybindingActionId = CoreKeybindingActionId | PluginOpenActionId | PluginRunActionId;
export type KeybindingOverrides = Partial<Record<KeybindingActionId, string | null>>;

const CORE_KEYBINDING_CATEGORIES: readonly KeybindingCategory[] = ["Workspace", "Panes", "Navigation", "Browser"];

/** Core's sections, then one for each plugin with shortcuts of its own. */
export function keybindingCategories(): readonly KeybindingCategory[] {
    return [
        ...CORE_KEYBINDING_CATEGORIES,
        ...enabledFrontendPlugins()
            .filter((plugin) => plugin.shortcuts?.length)
            .map(pluginCategory),
    ];
}

const PLUGIN_OPEN = "plugin.open:";
const PLUGIN_RUN = "plugin.run:";

function pluginCategory(plugin: FrontendPlugin): KeybindingCategory {
    return plugin.surfaces[0]?.title ?? plugin.id;
}

export function pluginRunAction(pluginId: string, name: string): PluginRunActionId {
    return `${PLUGIN_RUN}${pluginId}/${name}`;
}

/** The plugin shortcut an action runs, when it is one of those. */
export function pluginShortcutFor(id: string): PluginShortcut | null {
    if (!id.startsWith(PLUGIN_RUN)) return null;
    const [pluginId, name] = id.slice(PLUGIN_RUN.length).split("/");
    return frontendPlugin(pluginId)?.shortcuts?.find((shortcut) => shortcut.name === name) ?? null;
}

export function pluginOpenAction(pluginId: string): PluginOpenActionId {
    return `${PLUGIN_OPEN}${pluginId}`;
}

/** The plugin a shortcut opens, when it is one of those. */
export function pluginOpenedBy(id: string): string | null {
    return id.startsWith(PLUGIN_OPEN) ? id.slice(PLUGIN_OPEN.length) : null;
}

/** Core's actions, then each enabled plugin's: one to open it if it asks, and its own. */
export function keybindingActions(): readonly KeybindingAction[] {
    return [...coreKeybindingActions, ...enabledFrontendPlugins().flatMap(pluginActions)];
}

function pluginActions(plugin: FrontendPlugin): KeybindingAction[] {
    const opens: KeybindingAction[] = plugin.openShortcut
        ? [
              {
                  id: pluginOpenAction(plugin.id),
                  label: plugin.openTitle,
                  detail: `${plugin.openTitle}, or bring it forward`,
                  category: "Workspace",
                  defaultBinding: plugin.openShortcut,
              },
          ]
        : [];
    const own: KeybindingAction[] = (plugin.shortcuts ?? []).map((shortcut) => ({
        id: pluginRunAction(plugin.id, shortcut.name),
        label: shortcut.label,
        detail: shortcut.detail,
        category: pluginCategory(plugin),
        defaultBinding: shortcut.defaultBinding,
    }));
    return [...opens, ...own];
}

const MODIFIER_CODES = new Set(["MetaLeft", "MetaRight", "ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight"]);

export function getKeybindingAction(id: CoreKeybindingActionId): KeybindingAction;
export function getKeybindingAction(id: KeybindingActionId): KeybindingAction | undefined;
export function getKeybindingAction(id: KeybindingActionId): KeybindingAction | undefined {
    return keybindingActions().find((action) => action.id === id);
}

export function resolvedKeybinding(overrides: KeybindingOverrides, id: KeybindingActionId): string | null {
    const override = overrides[id];
    return override === undefined ? (getKeybindingAction(id)?.defaultBinding ?? null) : override;
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

/*
 * Every binding in force, keyed by the string a key press turns into.
 *
 * Rebuilt whenever the overrides change, which is when someone edits a
 * shortcut. Without it, answering "what does this key do?" walked all hundred
 * or so actions and built a binding string for each — on every single keydown,
 * including every character typed into a terminal.
 */
let bindingIndex: { overrides: KeybindingOverrides; actions: number; byBinding: Map<string, KeybindingActionId> } | null = null;

function keybindingIndex(overrides: KeybindingOverrides): Map<string, KeybindingActionId> {
    const actions = keybindingActions();
    if (bindingIndex?.overrides !== overrides || bindingIndex.actions !== actions.length) {
        const byBinding = new Map<string, KeybindingActionId>();
        for (const action of actions) {
            const binding = resolvedKeybinding(overrides, action.id as KeybindingActionId);
            // Declaration order decides a clash, which is what the scan this
            // replaces did by returning the first match.
            if (binding && !byBinding.has(binding)) byBinding.set(binding, action.id as KeybindingActionId);
        }
        bindingIndex = { overrides, actions: actions.length, byBinding };
    }
    return bindingIndex.byBinding;
}

export function actionForEvent(
    event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
    overrides: KeybindingOverrides,
): KeybindingActionId | null {
    const pressed = eventToKeybinding(event);
    if (!pressed) return null;
    const index = keybindingIndex(overrides);
    const direct = index.get(pressed);
    if (direct) return direct;
    // The main and numpad Enter keys are interchangeable for command shortcuts.
    if (event.code === "NumpadEnter") return index.get(pressed.replace(/\+NumpadEnter$/, "+Enter")) ?? null;
    // "+" is Shift and the "=" key, so a binding on plain Equal has to answer for both.
    if (event.shiftKey && event.code === "Equal") return index.get(pressed.replace(/\+Shift\+Equal$/, "+Equal")) ?? null;
    return null;
}

export function findKeybindingConflict(overrides: KeybindingOverrides, id: KeybindingActionId, binding: string): KeybindingAction | null {
    return (
        keybindingActions().find((action) => action.id !== id && resolvedKeybinding(overrides, action.id as KeybindingActionId) === binding) ?? null
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
    // A switched-off plugin keeps its rebound keys for when it is switched back on.
    const known = new Set([...coreKeybindingActions, ...frontendPlugins().flatMap(pluginActions)].map((action) => action.id));
    const out: KeybindingOverrides = {};
    for (const [id, binding] of Object.entries(value)) {
        if (!known.has(id)) continue;
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
