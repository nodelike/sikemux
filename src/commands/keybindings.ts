import { IS_MACOS } from "../lib/platform";
import { enabledFrontendPlugins } from "../plugins/enabled";
import { getState } from "../state/store";
import { frontendPlugin, frontendPlugins, type FrontendPlugin, type PluginShortcut } from "../plugins/registry";

type CoreKeybindingCategory = "Workspace" | "Agents" | "Panes" | "Navigation" | "Browser";
/** A plugin's own shortcuts are grouped under its name. */
export type KeybindingCategory = CoreKeybindingCategory | (string & {});

export interface KeybindingAction {
    id: string;
    label: string;
    detail: string;
    category: KeybindingCategory;
    /** `null` leaves the action unbound until someone records a key for it. */
    defaultBinding: string | null;
}

/*
 * Option is left to whatever has focus: shells and agent TUIs read it as Meta,
 * and other keyboard layouts type characters with it. So the macOS defaults
 * all hold Command or Control, and elsewhere Ctrl+Shift, which terminals leave
 * alone, stands in for Command.
 */
const on = (mac: string, other: string): string => (IS_MACOS ? mac : other);

const tabPositions = [1, 2, 3, 4, 5, 6, 7, 8, 9] as const;

const coreKeybindingActions = [
    {
        id: "palette.commands",
        label: "Open command deck",
        detail: "Search every action, shortcut, and custom command",
        category: "Workspace",
        defaultBinding: on("Meta+Shift+KeyP", "Ctrl+Shift+KeyP"),
    },
    {
        id: "palette.files",
        label: "Open file or request palette",
        detail: "Files in projects, or what the plugin in front offers",
        category: "Workspace",
        defaultBinding: on("Meta+KeyP", "Ctrl+Alt+KeyP"),
    },
    {
        id: "search.global",
        label: "Global search",
        detail: "Search across the active project",
        category: "Workspace",
        defaultBinding: on("Meta+Shift+KeyF", "Ctrl+Shift+KeyF"),
    },
    {
        id: "settings.toggle",
        label: "Open settings",
        detail: "Open or close this preferences window",
        category: "Workspace",
        defaultBinding: on("Meta+Comma", "Ctrl+Comma"),
    },
    {
        id: "view.focusMode",
        label: "Focus mode",
        detail: "Hide both rails",
        category: "Workspace",
        defaultBinding: on("Meta+KeyB", "Ctrl+Alt+KeyF"),
    },
    {
        id: "project.open",
        label: "Open project",
        detail: "Open the project picker",
        category: "Workspace",
        defaultBinding: on("Meta+KeyO", "Ctrl+Shift+KeyO"),
    },
    {
        id: "session.open",
        label: "Open project, host or plugin",
        detail: "Pick from projects, SSH hosts and plugins in one list",
        category: "Workspace",
        defaultBinding: on("Meta+Shift+KeyO", "Ctrl+Alt+KeyO"),
    },
    {
        id: "ssh.open",
        label: "Connect to SSH host",
        detail: "Open the SSH host picker",
        category: "Workspace",
        defaultBinding: on("Meta+Shift+KeyS", "Ctrl+Alt+KeyS"),
    },
    {
        id: "palette.newTab",
        label: "New…",
        detail: "Choose what to open: terminal, agent, browser, editor, Git or search",
        category: "Workspace",
        defaultBinding: null,
    },
    {
        id: "session.close",
        label: "Close session",
        detail: "Close the project, host or terminal session in front, asking first if agents would stop",
        category: "Workspace",
        defaultBinding: on("Meta+Shift+KeyW", "Ctrl+Shift+KeyQ"),
    },
    {
        id: "text.sizeIncrease",
        label: "Increase text size",
        detail: "Larger text in the focused editor or chat, or in every terminal",
        category: "Workspace",
        defaultBinding: on("Meta+Equal", "Ctrl+Equal"),
    },
    {
        id: "text.sizeDecrease",
        label: "Decrease text size",
        detail: "Smaller text in the focused editor or chat, or in every terminal",
        category: "Workspace",
        defaultBinding: on("Meta+Minus", "Ctrl+Minus"),
    },
    {
        id: "text.sizeReset",
        label: "Reset text size",
        detail: "Return the focused editor or chat, or every terminal, to its default size",
        category: "Workspace",
        defaultBinding: on("Meta+Digit0", "Ctrl+Digit0"),
    },
    {
        id: "agent.new",
        label: "New agent",
        detail: "Start the agent you launched last in this project, ready to type to",
        category: "Agents",
        defaultBinding: on("Meta+KeyN", "Ctrl+Shift+KeyN"),
    },
    {
        id: "agent.choose",
        label: "Choose agent",
        detail: "Start any installed agent, or resume an earlier conversation",
        category: "Agents",
        defaultBinding: on("Meta+Shift+KeyN", "Ctrl+Alt+KeyN"),
    },
    {
        id: "desk.toggle",
        label: "Show or hide desk",
        detail: "The agent's pages, files and terminals, beside it",
        category: "Agents",
        defaultBinding: on("Meta+KeyJ", "Ctrl+Shift+KeyJ"),
    },
    {
        id: "agent.permissions",
        label: "Toggle agent permissions",
        detail: "Toggle skip-permissions for the active agent",
        category: "Agents",
        defaultBinding: on("Meta+Shift+KeyY", "Ctrl+Shift+KeyY"),
    },
    {
        id: "terminal.new",
        label: "New terminal",
        detail: "A shell tab in this project, another login on this host, or a new command session",
        category: "Panes",
        defaultBinding: on("Meta+KeyT", "Ctrl+Shift+KeyT"),
    },
    {
        id: "session.command",
        label: "Focus command session",
        detail: "Jump to the command terminal",
        category: "Panes",
        defaultBinding: on("Meta+Ctrl+KeyT", "Ctrl+Alt+Shift+KeyT"),
    },
    {
        id: "pane.splitRow",
        label: "Split pane right",
        detail: "Create a side-by-side pane",
        category: "Panes",
        defaultBinding: on("Meta+KeyD", "Ctrl+Shift+KeyD"),
    },
    {
        id: "pane.splitColumn",
        label: "Split pane down",
        detail: "Create a pane below",
        category: "Panes",
        defaultBinding: on("Meta+Shift+KeyD", "Ctrl+Shift+KeyE"),
    },
    {
        id: "pane.splitStack",
        label: "Split pane into tabs",
        detail: "Create a pane in the same place, reached by tab",
        category: "Panes",
        defaultBinding: on("Meta+Alt+KeyD", "Ctrl+Alt+Shift+KeyD"),
    },
    {
        id: "pane.focusLeft",
        label: "Focus pane left",
        detail: "Move focus to the pane on the left",
        category: "Panes",
        defaultBinding: on("Meta+Alt+ArrowLeft", "Ctrl+Alt+ArrowLeft"),
    },
    {
        id: "pane.focusDown",
        label: "Focus pane down",
        detail: "Move focus to the pane below",
        category: "Panes",
        defaultBinding: on("Meta+Alt+ArrowDown", "Ctrl+Alt+ArrowDown"),
    },
    {
        id: "pane.focusUp",
        label: "Focus pane up",
        detail: "Move focus to the pane above",
        category: "Panes",
        defaultBinding: on("Meta+Alt+ArrowUp", "Ctrl+Alt+ArrowUp"),
    },
    {
        id: "pane.focusRight",
        label: "Focus pane right",
        detail: "Move focus to the pane on the right",
        category: "Panes",
        defaultBinding: on("Meta+Alt+ArrowRight", "Ctrl+Alt+ArrowRight"),
    },
    {
        id: "pane.resizeLeft",
        label: "Resize pane left",
        detail: "Grow the active pane toward the left",
        category: "Panes",
        defaultBinding: on("Meta+Ctrl+ArrowLeft", "Ctrl+Alt+Shift+ArrowLeft"),
    },
    {
        id: "pane.resizeDown",
        label: "Resize pane down",
        detail: "Grow the active pane downward",
        category: "Panes",
        defaultBinding: on("Meta+Ctrl+ArrowDown", "Ctrl+Alt+Shift+ArrowDown"),
    },
    {
        id: "pane.resizeUp",
        label: "Resize pane up",
        detail: "Grow the active pane upward",
        category: "Panes",
        defaultBinding: on("Meta+Ctrl+ArrowUp", "Ctrl+Alt+Shift+ArrowUp"),
    },
    {
        id: "pane.resizeRight",
        label: "Resize pane right",
        detail: "Grow the active pane toward the right",
        category: "Panes",
        defaultBinding: on("Meta+Ctrl+ArrowRight", "Ctrl+Alt+Shift+ArrowRight"),
    },
    {
        id: "pane.zoom",
        label: "Zoom pane",
        detail: "Fill the window with the active pane",
        category: "Panes",
        defaultBinding: on("Meta+Shift+Enter", "Ctrl+Shift+Enter"),
    },
    {
        id: "pane.close",
        label: "Close",
        detail: "Close the page, file, pane or tab in front, asking first if an agent is mid-turn",
        category: "Panes",
        defaultBinding: on("Meta+KeyW", "Ctrl+Shift+KeyW"),
    },
    {
        id: "window.previous",
        label: "Previous tab",
        detail: "Move one tab left along the strip",
        category: "Navigation",
        defaultBinding: on("Meta+Shift+BracketLeft", "Ctrl+PageUp"),
    },
    {
        id: "window.next",
        label: "Next tab",
        detail: "Move one tab right along the strip",
        category: "Navigation",
        defaultBinding: on("Meta+Shift+BracketRight", "Ctrl+PageDown"),
    },
    {
        id: "tab.previous",
        label: "Previous of the same kind",
        detail: "The previous desk page, agent, file or terminal, whichever is in front",
        category: "Navigation",
        defaultBinding: "Ctrl+Shift+Tab",
    },
    {
        id: "tab.next",
        label: "Next of the same kind",
        detail: "The next desk page, agent, file or terminal, whichever is in front",
        category: "Navigation",
        defaultBinding: "Ctrl+Tab",
    },
    ...tabPositions.map(
        (position) =>
            ({
                id: `tab.goto${position}`,
                label: position === 9 ? "Last tab" : `Tab ${position}`,
                detail: position === 9 ? "Jump to the last tab in the strip" : `Jump to tab ${position} in the strip`,
                category: "Navigation",
                defaultBinding: on(`Meta+Digit${position}`, `Ctrl+Digit${position}`),
            }) as const,
    ),
    {
        id: "session.lastUsed",
        label: "Switch to last-used session",
        detail: "Toggle back to the session you used immediately before this one",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+KeyU", "Ctrl+Alt+KeyU"),
    },
    {
        id: "session.next",
        label: "Next session",
        detail: "Cycle forward through sessions; hold Control to keep choosing",
        category: "Navigation",
        defaultBinding: "Ctrl+Backquote",
    },
    {
        id: "session.previous",
        label: "Previous session",
        detail: "Cycle backward through sessions",
        category: "Navigation",
        defaultBinding: "Ctrl+Shift+Backquote",
    },
    {
        id: "session.nextGroup",
        label: "Next session group",
        detail: "Cycle through project, SSH, cloud and command groups",
        category: "Navigation",
        defaultBinding: "Ctrl+Alt+Backquote",
    },
    {
        id: "window.files",
        label: "Focus files",
        detail: "Jump to the files window",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+Digit1", "Ctrl+Alt+Digit1"),
    },
    {
        id: "window.terminal",
        label: "Focus terminal",
        detail: "Jump to the terminal window",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+Digit2", "Ctrl+Alt+Digit2"),
    },
    {
        id: "window.git",
        label: "Focus Git",
        detail: "Jump to the Git window",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+Digit3", "Ctrl+Alt+Digit3"),
    },
    {
        id: "window.agents",
        label: "Focus agents",
        detail: "Jump to the agents view",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+Digit4", "Ctrl+Alt+Digit4"),
    },
    {
        id: "window.search",
        label: "Focus search",
        detail: "Jump to the search window",
        category: "Navigation",
        defaultBinding: on("Meta+Alt+Digit5", "Ctrl+Alt+Digit5"),
    },
    {
        id: "browser.tabNew",
        label: "New browser tab",
        detail: "Open a page on the desk of the agent in front, or the one that worked last",
        category: "Browser",
        defaultBinding: on("Meta+Shift+KeyT", "Ctrl+Shift+KeyB"),
    },
    {
        id: "browser.address",
        label: "Focus browser address",
        detail: "Focus the embedded browser address bar",
        category: "Browser",
        defaultBinding: on("Meta+KeyL", "Ctrl+Shift+KeyL"),
    },
    {
        id: "browser.reload",
        label: "Reload browser tab",
        detail: "Reload the active embedded browser tab",
        category: "Browser",
        defaultBinding: on("Meta+KeyR", "Ctrl+KeyR"),
    },
    {
        id: "browser.back",
        label: "Browser back",
        detail: "Go back in the active embedded browser tab",
        category: "Browser",
        defaultBinding: on("Meta+BracketLeft", "Ctrl+BracketLeft"),
    },
    {
        id: "browser.forward",
        label: "Browser forward",
        detail: "Go forward in the active embedded browser tab",
        category: "Browser",
        defaultBinding: on("Meta+BracketRight", "Ctrl+BracketRight"),
    },
] as const satisfies readonly KeybindingAction[];

export type CoreKeybindingActionId = (typeof coreKeybindingActions)[number]["id"];
/** Opens a plugin, for plugins that ask for a shortcut. */
export type PluginOpenActionId = `plugin.open:${string}`;
/** One of a plugin's own shortcuts, as `plugin.run:<plugin id>/<name>`. */
export type PluginRunActionId = `plugin.run:${string}`;
export type KeybindingActionId = CoreKeybindingActionId | PluginOpenActionId | PluginRunActionId;
export type KeybindingOverrides = Partial<Record<KeybindingActionId, string | null>>;

const CORE_KEYBINDING_CATEGORIES: readonly KeybindingCategory[] = ["Workspace", "Agents", "Panes", "Navigation", "Browser"];

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
    return getState().pluginManifests.find((manifest) => manifest.id === plugin.id)?.name ?? plugin.id;
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

/** Shift alone only changes what a key types, so a shortcut needs one of the others. */
export function keybindingHasModifier(binding: string): boolean {
    const parts = binding.split("+");
    return parts.includes("Meta") || parts.includes("Ctrl") || parts.includes("Alt");
}

const RESERVED: Readonly<Record<string, string>> = IS_MACOS
    ? {
          "Meta+KeyQ": "Quit",
          "Meta+KeyH": "Hide",
          "Meta+Alt+KeyH": "Hide Others",
          "Meta+KeyM": "Minimize",
          "Meta+Tab": "the app switcher",
          "Meta+Space": "Spotlight",
          "Meta+KeyC": "Copy",
          "Meta+KeyV": "Paste",
          "Meta+KeyX": "Cut",
          "Meta+KeyA": "Select All",
          "Meta+KeyZ": "Undo",
          "Meta+Shift+KeyZ": "Redo",
      }
    : { "Alt+Tab": "the window switcher", "Alt+F4": "closing the window", "Ctrl+Shift+KeyC": "Copy", "Ctrl+Shift+KeyV": "Paste" };

/** What the system or every text field already does with this key, when that is taken. */
export function reservedKeybinding(binding: string): string | null {
    return RESERVED[binding] ?? null;
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
