import {
    keybindingActions,
    keybindingLabelForAction,
    type KeybindingActionId,
    type KeybindingCategory,
    type KeybindingOverrides,
} from "../keybindings";
import type { SessionKind } from "../state/types";

export type CommandContext = SessionKind;

export type CustomCommandPlacement = "background" | "terminal" | "split" | "popup" | "replace";

/**
 * A trusted, user-authored shell command. Execution and environment injection
 * are deliberately owned by the caller; the registry only handles discovery,
 * context filtering, and dispatch.
 */
export interface CustomCommand {
    id: string;
    title: string;
    detail: string;
    command: string;
    /** Empty means the command is available in every session context. */
    contexts: CommandContext[];
    placement: CustomCommandPlacement;
}

export type BuiltinCommandExecutor = (id: KeybindingActionId) => void;
export type CustomCommandExecutor = (command: CustomCommand) => void;

/** A first-party action that is intentionally available without a keybinding. */
export interface StandaloneCommand {
    id: string;
    title: string;
    detail: string;
    category: string;
    /** Optional display-only binding; dispatch remains owned by the caller. */
    shortcut?: string;
    /** Disabled commands remain discoverable but cannot be dispatched. */
    disabled?: boolean;
    execute: () => void;
}

export interface BuiltinCommandEntry {
    kind: "builtin";
    key: string;
    id: KeybindingActionId;
    title: string;
    detail: string;
    category: KeybindingCategory;
    shortcut: string;
    searchText: string;
    execute: () => void;
}

export interface CustomCommandEntry {
    kind: "custom";
    key: string;
    id: string;
    title: string;
    detail: string;
    category: "Custom";
    shortcut: "";
    placement: CustomCommandPlacement;
    searchText: string;
    command: CustomCommand;
    execute: () => void;
}

export interface StandaloneCommandEntry {
    kind: "standalone";
    key: string;
    id: string;
    title: string;
    detail: string;
    category: string;
    shortcut: string;
    disabled: boolean;
    searchText: string;
    execute: () => void;
}

export type CommandEntry = BuiltinCommandEntry | StandaloneCommandEntry | CustomCommandEntry;

export interface BuildCommandRegistryOptions {
    keybindingOverrides: KeybindingOverrides;
    executeBuiltin: BuiltinCommandExecutor;
    customCommands?: readonly CustomCommand[];
    executeCustom?: CustomCommandExecutor;
    context?: CommandContext | null;
    standaloneCommands?: readonly StandaloneCommand[];
}

export function customCommandAvailable(command: CustomCommand, context?: CommandContext | null): boolean {
    return command.contexts.length === 0 || (!!context && command.contexts.includes(context));
}

export function buildBuiltinCommandEntries(keybindingOverrides: KeybindingOverrides, executeBuiltin: BuiltinCommandExecutor): BuiltinCommandEntry[] {
    return keybindingActions().map((action) => {
        const id = action.id as KeybindingActionId;
        const shortcut = keybindingLabelForAction(keybindingOverrides, id);
        return {
            kind: "builtin",
            key: `builtin:${id}`,
            id,
            title: action.label,
            detail: action.detail,
            category: action.category,
            shortcut,
            searchText: `${action.label} ${action.detail} ${action.category} ${shortcut} ${id}`,
            execute: () => executeBuiltin(id),
        };
    });
}

export function buildCommandRegistry({
    keybindingOverrides,
    executeBuiltin,
    customCommands = [],
    executeCustom,
    context = null,
    standaloneCommands = [],
}: BuildCommandRegistryOptions): CommandEntry[] {
    const builtins = buildBuiltinCommandEntries(keybindingOverrides, executeBuiltin);
    const standalone: StandaloneCommandEntry[] = standaloneCommands.map((command) => ({
        kind: "standalone",
        key: `standalone:${command.id}`,
        id: command.id,
        title: command.title,
        detail: command.detail,
        category: command.category,
        shortcut: command.shortcut ?? "",
        disabled: command.disabled ?? false,
        searchText: `${command.title} ${command.detail} ${command.category} ${command.shortcut ?? ""} ${command.id}`,
        execute: command.execute,
    }));
    if (!executeCustom) return [...builtins, ...standalone];

    const custom: CustomCommandEntry[] = customCommands
        .filter((command) => customCommandAvailable(command, context))
        .map((command) => ({
            kind: "custom",
            key: `custom:${command.id}`,
            id: command.id,
            title: command.title,
            detail: command.detail,
            category: "Custom",
            shortcut: "",
            placement: command.placement,
            command,
            searchText: `${command.title} ${command.detail} ${command.command} ${command.placement} ${command.contexts.join(" ")}`,
            execute: () => executeCustom(command),
        }));

    return [...builtins, ...standalone, ...custom];
}
