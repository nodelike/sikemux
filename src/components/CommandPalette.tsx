import { useEffect, useMemo, useRef, useState } from "react";
import {
    buildCommandRegistry,
    type BuiltinCommandExecutor,
    type CommandContext,
    type CommandEntry,
    type CustomCommand,
    type CustomCommandExecutor,
    type StandaloneCommand,
} from "../commands/registry";
import { useMouseActive } from "../hooks/useMouseActive";
import { rankBy } from "../lib/fuzzy";
import type { KeybindingOverrides } from "../keybindings";
import { IconCommand, IconSearch } from "./Icons";

const MAX_RESULTS = 200;

export interface CommandPaletteProps {
    keybindingOverrides: KeybindingOverrides;
    executeBuiltin: BuiltinCommandExecutor;
    customCommands?: readonly CustomCommand[];
    executeCustom?: CustomCommandExecutor;
    context?: CommandContext | null;
    onClose: () => void;
    onExecute?: (key: string) => void;
    recentCommandKeys?: readonly string[];
    standaloneCommands?: readonly StandaloneCommand[];
}

function entryMeta(entry: CommandEntry): string {
    if (entry.kind === "custom") return `custom · ${entry.placement}`;
    return entry.category;
}

export function CommandPalette({
    keybindingOverrides,
    executeBuiltin,
    customCommands = [],
    executeCustom,
    context = null,
    onClose,
    onExecute,
    recentCommandKeys = [],
    standaloneCommands = [],
}: CommandPaletteProps) {
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();

    const registry = useMemo(() => {
        const rows = buildCommandRegistry({ keybindingOverrides, executeBuiltin, customCommands, executeCustom, context, standaloneCommands });
        const recent = new Map(recentCommandKeys.map((key, index) => [key, index]));
        return rows.sort((a, b) => (recent.get(a.key) ?? Number.MAX_SAFE_INTEGER) - (recent.get(b.key) ?? Number.MAX_SAFE_INTEGER));
    }, [context, customCommands, executeBuiltin, executeCustom, keybindingOverrides, recentCommandKeys, standaloneCommands]);
    const entries = useMemo(() => rankBy(query, registry, (entry) => entry.searchText).slice(0, MAX_RESULTS), [query, registry]);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    useEffect(() => {
        setSelected(0);
    }, [query]);

    useEffect(() => {
        setSelected((current) => Math.min(current, Math.max(0, entries.length - 1)));
    }, [entries.length]);

    useEffect(() => {
        const row = listRef.current?.querySelector<HTMLElement>(`.command-palette-item:nth-child(${selected + 1})`);
        row?.scrollIntoView?.({ block: "nearest" });
    }, [selected]);

    const activate = (entry: CommandEntry | undefined) => {
        if (!entry) return;
        onClose();
        onExecute?.(entry.key);
        entry.execute();
    };

    const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
        if (event.key === "Escape") {
            event.preventDefault();
            onClose();
        } else if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            setSelected((current) => (entries.length ? (current + 1) % entries.length : 0));
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            setSelected((current) => (entries.length ? (current - 1 + entries.length) % entries.length : 0));
        } else if (event.key === "Enter") {
            event.preventDefault();
            activate(entries[selected]);
        }
    };

    return (
        <div className="picker-backdrop command-palette-backdrop" onMouseDown={onClose}>
            <div
                className="picker command-palette"
                role="dialog"
                aria-modal="true"
                aria-label="Command palette"
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        aria-label="Search commands"
                        placeholder="type a command…"
                        value={query}
                        onChange={(event) => setQuery(event.target.value)}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                    <span className="picker-hints" aria-hidden="true">
                        <span className="picker-hint">↑↓ nav</span>
                        <span className="picker-hint">⏎ run</span>
                        <span className="picker-hint">esc</span>
                    </span>
                </div>

                <div className="picker-list command-palette-list" ref={listRef} role="listbox" aria-label="Commands">
                    {entries.length === 0 && <div className="picker-empty">no commands match</div>}
                    {entries.map((entry, index) => (
                        <button
                            key={entry.key}
                            type="button"
                            role="option"
                            aria-selected={index === selected}
                            className={`picker-item command-palette-item${index === selected ? " sel" : ""}`}
                            onMouseEnter={() => {
                                if (mouseActive.current) setSelected(index);
                            }}
                            onClick={() => activate(entry)}>
                            <span className={`picker-icon command-palette-icon ${entry.kind}`} aria-hidden="true">
                                <IconCommand size={14} />
                            </span>
                            <span className="command-palette-copy">
                                <span className="picker-name">{entry.title}</span>
                                <span className="picker-sub">{entry.detail}</span>
                            </span>
                            <span className="command-palette-meta">{entryMeta(entry)}</span>
                            {entry.shortcut && <kbd className="command-palette-shortcut">{entry.shortcut}</kbd>}
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
