import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { AgentIcon, IconCheck, IconChevron } from "../components/Icons";
import { useStore } from "../state/store";
import { DEFAULT_PROVIDER_PROFILE_SELECTION, type Agent, type ProviderProfile } from "../state/types";

interface Choice {
    value: string;
    label: string;
    description?: string;
    icon?: ReactNode;
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

const HARNESSES = [
    { type: "codex", label: "Codex" },
    { type: "claude", label: "Claude" },
] as const;

const NAMED_VERSION = /^(\p{L}+)\s+(\d+(?:\.\d+)?)\b/u;

// The agent names a model without its release number ("Opus") and leaves that
// number in the description ("Opus 5 with 1M context"), so put it back.
function versioned(label: string, description?: string): string {
    const named = description?.match(NAMED_VERSION);
    if (!named) return label;
    const [, family, version] = named;
    const head = label.split(" ")[0];
    return head.toLowerCase() !== family.toLowerCase() || label.includes(version) ? label : label.replace(head, `${head} ${version}`);
}

function choices(value: unknown, group?: string): Choice[] {
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): Choice[] => {
        const row = record(item);
        if (!row) return [];
        if (Array.isArray(row.options)) return choices(row.options, typeof row.name === "string" ? row.name : undefined);
        if (typeof row.value !== "string" || typeof row.name !== "string") return [];
        const description = typeof row.description === "string" ? row.description : group;
        return [{ value: row.value, label: versioned(row.name, description), description }];
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
    header,
    loading = false,
    onOpen,
    compact = false,
}: {
    name: string;
    label: string;
    value: string;
    options: Choice[];
    disabled: boolean;
    onSelect: (value: string) => void;
    icon?: ReactNode;
    header?: ReactNode;
    /** Holds the menu open while the control is disabled, with the list waiting on new options. */
    loading?: boolean;
    onOpen?: () => void;
    /** Descriptions stay searchable but go unrendered, so the rows read as one line. */
    compact?: boolean;
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
    const shown = open && (!disabled || loading);
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
                // WebKit hands focus to nobody when a button is clicked, so a blur
                // with no new target is a click on our own menu, not a click away.
                if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
            }}>
            <button
                ref={trigger}
                type="button"
                className="chat-picker-trigger"
                aria-label={name}
                aria-haspopup="listbox"
                aria-expanded={shown}
                disabled={disabled}
                title={label}
                onClick={() => {
                    if (!open) onOpen?.();
                    setOpen(!open);
                    setQuery("");
                    setSelected(0);
                }}>
                {icon}
                <span>{label}</span>
                <IconChevron size={10} />
            </button>
            {shown && (
                <div
                    className={`chat-picker-menu${compact ? " compact" : ""}`}
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
                            if (event.key === "Enter" && !event.nativeEvent.isComposing && !loading && filtered[selected]) {
                                event.preventDefault();
                                onSelect(filtered[selected].value);
                                close();
                            }
                        }}
                    />
                    {header}
                    <div id={listId} role="listbox" aria-label={`${name} options`} className="chat-picker-options">
                        {loading && <div className="chat-picker-hint">Loading models…</div>}
                        {!loading &&
                            filtered.map((option, index) => (
                                <button
                                    type="button"
                                    key={option.value}
                                    id={`${listId}-${index}`}
                                    role="option"
                                    aria-selected={option.value === value}
                                    className={index === selected ? "highlighted" : ""}
                                    onMouseEnter={() => setSelected(index)}
                                    onMouseDown={(event) => event.preventDefault()}
                                    onClick={() => {
                                        onSelect(option.value);
                                        close();
                                    }}>
                                    {option.icon}
                                    <span>
                                        <strong>{option.label}</strong>
                                        {option.description && !compact && <small>{option.description}</small>}
                                    </span>
                                    {option.value === value && <IconCheck size={14} />}
                                </button>
                            ))}
                        {!loading && filtered.length === 0 && <div className="chat-picker-hint">No matches</div>}
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
    const agentOptions = HARNESSES.flatMap(({ type, label }) => {
        const icon = <AgentIcon type={type} size={15} className={`agent-glyph ${type}`} />;
        const owned = profiles.filter((item) => item.provider === type);
        if (owned.length === 0) return [{ value: type, label, icon }];
        return owned.map((item) => ({ value: item.id, label: item.name, icon }));
    });
    const builtin = DEFAULT_PROVIDER_PROFILE_SELECTION[agent.type];
    const agentValue = profile?.id ?? (builtin && agentOptions.some((option) => option.value === builtin) ? builtin : agent.type);
    const agentIcon = <AgentIcon type={agent.type} size={17} className={`agent-glyph ${agent.type}`} />;
    const rowIcon = <AgentIcon type={agent.type} size={15} className={`agent-glyph ${agent.type}`} />;
    const [switching, setSwitching] = useState<Record<string, unknown>>();
    const loading = switching !== undefined && (disabled || switching === setup);
    useEffect(() => {
        if (switching && !loading) setSwitching(undefined);
    }, [switching, loading]);
    const agentRow = (
        <div className="chat-picker-agents" role="group" aria-label="Agent">
            {agentOptions.map((option) => (
                <button
                    type="button"
                    key={option.value}
                    aria-pressed={option.value === agentValue}
                    title={option.label}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => {
                        if (option.value === agentValue) return;
                        const next = profiles.find((item) => item.id === option.value);
                        const type = next?.provider ?? option.value;
                        if (type !== "claude" && type !== "codex") return;
                        setSwitching(setup);
                        onAgent(type, next?.id);
                    }}>
                    {option.icon}
                    <span>{option.label}</span>
                </button>
            ))}
        </div>
    );
    return (
        <div className="chat-pickers">
            {["model", agent.type === "claude" ? "effort" : "reasoning_effort"].map((id) => {
                const config = configs.find((item) => item.id === id);
                const model = id === "model";
                const name = model ? "Model" : "Reasoning effort";
                const label =
                    config?.options.find((option) => option.value === config.currentValue)?.label ??
                    config?.currentValue ??
                    (model ? agent.model || "Model" : agent.effort || "Effort");
                const options = config?.options || [];
                return (
                    <Picker
                        key={id}
                        name={name}
                        label={label}
                        value={config?.currentValue || ""}
                        options={model ? options.map((option) => ({ ...option, icon: rowIcon })) : options}
                        disabled={disabled || (!options.length && (!model || agentLocked))}
                        icon={model ? agentIcon : undefined}
                        header={model && !agentLocked ? agentRow : undefined}
                        loading={model && loading}
                        onOpen={model ? () => setSwitching(undefined) : undefined}
                        compact={model}
                        onSelect={(value) => {
                            if (config && value !== config.currentValue) onConfig(config, value);
                        }}
                    />
                );
            })}
        </div>
    );
}
