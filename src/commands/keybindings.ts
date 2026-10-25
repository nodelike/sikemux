import { IS_MACOS } from "@shared/lib/platform";
import { enabledFrontendPlugins } from "../plugins/enabled";
import { getState } from "../state/store";
import { frontendPlugin, frontendPlugins, type FrontendPlugin, type PluginShortcut } from "../plugins/registry";
import { preferencesOf } from "../state/preferences";
import type { StoreState } from "../state/store";
import { CORE_KEYBINDINGS } from "./generated/keybindings";

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

const coreKeybindingActions = CORE_KEYBINDINGS.map(({ mac, other, ...action }) => ({
    ...action,
    defaultBinding: mac === null ? null : on(mac, other ?? mac),
})) satisfies readonly KeybindingAction[];

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

export const ENTER_GLYPH = "↵";

const MODIFIER_GLYPHS: Record<KeybindingModifier, string> = { Meta: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧" };

export type KeybindingModifier = "Meta" | "Ctrl" | "Alt" | "Shift";

/** A modifier as shortcuts show it: the Mac's glyph, or its name elsewhere. */
export function modifierGlyph(modifier: KeybindingModifier): string {
    return IS_MACOS ? MODIFIER_GLYPHS[modifier] : modifier;
}

const CODE_LABELS: Record<string, string> = {
    Backquote: "`",
    Backslash: "\\",
    BracketLeft: "[",
    BracketRight: "]",
    Comma: ",",
    Enter: ENTER_GLYPH,
    Equal: "=",
    Escape: "Esc",
    Minus: "-",
    NumpadEnter: `Num ${ENTER_GLYPH}`,
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

/** A binding such as "Alt+Meta+KeyQ" as shortcuts show it: ⌥⌘Q on a Mac, Alt+Meta+Q elsewhere. */
export function shortcutText(binding: string): string {
    const parts = binding.split("+");
    const code = parts.pop() ?? "";
    const modifiers = parts.map((part) => (part in MODIFIER_GLYPHS ? modifierGlyph(part as KeybindingModifier) : part));
    return [...modifiers, codeLabel(code)].join(IS_MACOS ? "" : "+");
}

export function keybindingLabel(binding: string | null): string {
    return binding ? shortcutText(binding) : "Unassigned";
}

/** Quit and Stop Everything is on the app menu, which only the Mac has. */
export const QUIT_AND_STOP_SHORTCUT = IS_MACOS ? shortcutText("Alt+Meta+KeyQ") : null;

export function keybindingLabelForAction(overrides: KeybindingOverrides, id: KeybindingActionId): string {
    return keybindingLabel(resolvedKeybinding(overrides, id));
}

let readOverrides: unknown;
let usableOverrides: KeybindingOverrides = {};

/** The person's rebound keys that name an action and a key with a modifier, kept until they change. */
export function keybindingOverridesOf(state: StoreState): KeybindingOverrides {
    const overrides = preferencesOf(state).keybindingOverrides;
    if (overrides !== readOverrides) {
        readOverrides = overrides;
        usableOverrides = normaliseKeybindingOverrides(overrides);
    }
    return usableOverrides;
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
