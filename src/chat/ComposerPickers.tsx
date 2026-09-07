import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { AgentIcon, IconCheck, IconChevron } from "../components/Icons";
import { useStore } from "../state/store";
import type { Agent, ProviderProfile } from "../state/types";

interface Choice {
    value: string;
    label: string;
    description?: string;
}

export interface SessionConfig {
    id: string;
    name: string;
    currentValue: string;
    options: Choice[];
}

function record(value: unknown): Record<string, unknown> | null {
    return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function choices(value: unknown, group?: string): Choice[] {
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): Choice[] => {
        const row = record(item);
        if (!row) return [];
        if (Array.isArray(row.options)) return choices(row.options, typeof row.name === "string" ? row.name : undefined);
        return typeof row.value === "string" && typeof row.name === "string"
            ? [{ value: row.value, label: row.name, description: typeof row.description === "string" ? row.description : group }]
            : [];
    });
}

export function sessionConfigs(setup: Record<string, unknown>): SessionConfig[] {
    if (!Array.isArray(setup.configOptions)) return [];
    return setup.configOptions.flatMap((value): SessionConfig[] => {
        const row = record(value);
        if (!row || row.type !== "select" || typeof row.id !== "string" || typeof row.currentValue !== "string") return [];
        return [
            { id: row.id, name: typeof row.name === "string" ? row.name : row.id, currentValue: row.currentValue, options: choices(row.options) },
        ];
    });
}

function Picker({
    name,
    label,
    value,
    options,
    disabled,
    onSelect,
    icon,
    hint,
}: {
    name: string;
    label: string;
    value: string;
    options: Choice[];
    disabled: boolean;
    onSelect: (value: string) => void;
    icon?: ReactNode;
    hint?: string;
}) {
    const [open, setOpen] = useState(false);
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const root = useRef<HTMLDivElement>(null);
    const trigger = useRef<HTMLButtonElement>(null);
    const search = useRef<HTMLInputElement>(null);
    const listId = useId();
    const filtered = options.filter((option) =>
        [option.label, option.value, option.description].join(" ").toLowerCase().includes(query.toLowerCase()),
    );
    const close = () => {
        setOpen(false);
        trigger.current?.focus();
    };

    useEffect(() => {
        if (!open) return;
        search.current?.focus();
        const outside = (event: PointerEvent) => {
            if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
        };
        document.addEventListener("pointerdown", outside);
        return () => document.removeEventListener("pointerdown", outside);
    }, [open]);

    return (
        <div
            className="chat-picker"
            ref={root}
            onBlur={(event) => {
                if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
            }}>
            <button
                ref={trigger}
                type="button"
                className="chat-picker-trigger"
                aria-label={name}
                aria-haspopup="listbox"
                aria-expanded={open && !disabled}
                disabled={disabled}
                title={disabled && hint ? hint : label}
                onClick={() => {
                    setOpen(!open);
                    setQuery("");
                    setSelected(0);
                }}>
                {icon}
                <span>{label}</span>
                <IconChevron size={10} />
            </button>
            {open && !disabled && (
                <div
                    className="chat-picker-menu"
                    style={{
                        bottom: window.innerWidth <= 650 ? window.innerHeight - (trigger.current?.getBoundingClientRect().top ?? 0) + 12 : undefined,
                    }}>
                    <input
                        ref={search}
                        value={query}
                        aria-label={`Search ${name.toLowerCase()}`}
                        placeholder={`Search ${name.toLowerCase()}…`}
                        role="combobox"
                        aria-controls={listId}
                        aria-expanded
                        aria-autocomplete="list"
                        aria-activedescendant={filtered[selected] ? `${listId}-${selected}` : undefined}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSelected(0);
                        }}
                        onKeyDown={(event) => {
                            if (event.key === "Escape") {
                                event.preventDefault();
                                event.stopPropagation();
                                close();
                            }
                            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                                event.preventDefault();
                                setSelected((index) =>
                                    filtered.length ? (index + (event.key === "ArrowDown" ? 1 : filtered.length - 1)) % filtered.length : 0,
                                );
                            }
                            if (event.key === "Enter" && !event.nativeEvent.isComposing && filtered[selected]) {
                                event.preventDefault();
                                onSelect(filtered[selected].value);
                                close();
                            }
                        }}
                    />
                    {hint && <div className="chat-picker-hint">{hint}</div>}
                    <div id={listId} role="listbox" aria-label={`${name} options`} className="chat-picker-options">
                        {filtered.map((option, index) => (
                            <button
                                type="button"
                                key={option.value}
                                id={`${listId}-${index}`}
                                role="option"
                                aria-selected={option.value === value}
                                className={index === selected ? "highlighted" : ""}
                                onMouseEnter={() => setSelected(index)}
                                onClick={() => {
                                    onSelect(option.value);
                                    close();
                                }}>
                                <span>
                                    <strong>{option.label}</strong>
                                    {option.description && <small>{option.description}</small>}
                                </span>
                                {option.value === value && <IconCheck size={14} />}
                            </button>
                        ))}
                        {filtered.length === 0 && <div className="chat-picker-hint">No matches</div>}
                    </div>
                </div>
            )}
        </div>
    );
}

export function ComposerPickers({
    agent,
    profile,
    setup,
    disabled,
    agentLocked = false,
    onConfig,
    onAgent,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    setup: Record<string, unknown>;
    disabled: boolean;
    agentLocked?: boolean;
    onAgent: (type: "codex" | "claude", profileId?: string) => void;
    onConfig: (config: SessionConfig, value: string) => void;
}) {
    const profiles = useStore((state) => state.providerProfiles);
    const configs = sessionConfigs(setup);
    const agentOptions = [
        { value: "codex", label: "Codex", description: "Default configuration" },
        { value: "claude", label: "Claude", description: "Default configuration" },
        ...profiles
            .filter((item) => item.provider === "codex" || item.provider === "claude")
            .map((item) => ({ value: item.id, label: item.name, description: item.provider === "codex" ? "Codex" : "Claude" })),
    ];
    return (
        <div className="chat-pickers">
            <Picker
                name="Agent"
                label={profile?.name || (agent.type === "codex" ? "Codex" : "Claude")}
                value={profile?.id || agent.type}
                options={agentOptions}
                disabled={disabled || agentLocked}
                icon={<AgentIcon type={agent.type} size={15} />}
                hint={agentLocked ? "The agent is fixed after the first message." : "Choose the harness for this chat."}
                onSelect={(value) => {
                    if (agentLocked || value === (profile?.id || agent.type)) return;
                    const next = profiles.find((item) => item.id === value);
                    const type = next?.provider ?? value;
                    if (type !== "claude" && type !== "codex") return;
                    onAgent(type, next?.id);
                }}
            />
            {["model", agent.type === "claude" ? "effort" : "reasoning_effort"].map((id) => {
                const config = configs.find((item) => item.id === id);
                const name = id === "model" ? "Model" : "Reasoning effort";
                const label =
                    config?.options.find((option) => option.value === config.currentValue)?.label ??
                    config?.currentValue ??
                    (id === "model" ? agent.model || "Model" : agent.effort || "Effort");
                return (
                    <Picker
                        key={id}
                        name={name}
                        label={label}
                        value={config?.currentValue || ""}
                        options={config?.options || []}
                        disabled={disabled || !config?.options.length}
                        onSelect={(value) => {
                            if (config && value !== config.currentValue) onConfig(config, value);
                        }}
                    />
                );
            })}
        </div>
    );
}
