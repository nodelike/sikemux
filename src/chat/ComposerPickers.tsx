import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { AgentIcon, IconCheck, IconChevron } from "../ui/Icons";
import { agentSupportsChat, CHAT_AGENT_TYPES, type ChatAgentType } from "../agents/agentLaunch";
import { selectedAgentRuntimeProfiles } from "../agents/agentProfiles";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { DEFAULT_PROVIDER_PROFILE_SELECTION, type Agent, type ProviderProfile } from "../state/types";
import { leavingMenu } from "../lib/motion";
import { pickerSlots, sessionConfigs, type ConfigChoice, type SessionConfig } from "./sessionConfig";

interface Choice extends ConfigChoice {
    icon?: ReactNode;
}

const HARNESS_LABELS: Record<ChatAgentType, string> = {
    codex: "Codex",
    claude: "Claude",
    opencode: "OpenCode",
    omp: "OMP",
    grok: "Grok",
    hermes: "Hermes",
};

/* Claude and Codex install their adapter on first use, so they are offered
   whether or not their CLI is found yet. The rest need their own CLI. */
const ALWAYS_OFFERED = new Set<ChatAgentType>(["codex", "claude"]);

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
                    ref={leavingMenu}
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
    onAgent: (type: ChatAgentType, profileId?: string) => void;
    onConfig: (config: SessionConfig, value: string) => void;
}) {
    const profiles = useStore((state) => state.providerProfiles);
    const profileSelections = useStore((state) => state.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const installed = new Set((catalog.data ?? []).filter((item) => item.available !== false).map((item) => item.type));
    const configs = sessionConfigs(setup);
    const harnesses = CHAT_AGENT_TYPES.filter((type) => ALWAYS_OFFERED.has(type) || type === agent.type || installed.has(type));
    const agentOptions = harnesses.flatMap((type) => {
        const label = HARNESS_LABELS[type];
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
                        if (!agentSupportsChat(type as Agent["type"])) return;
                        setSwitching(setup);
                        onAgent(type as ChatAgentType, next?.id);
                    }}>
                    {option.icon}
                    <span>{option.label}</span>
                </button>
            ))}
        </div>
    );
    return (
        <div className="chat-pickers">
            {pickerSlots(configs, agent.type).map(({ id, config }) => {
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
