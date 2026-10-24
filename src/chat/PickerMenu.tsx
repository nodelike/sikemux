import { useEffect, useId, useMemo, useRef, type Dispatch, type ReactNode, type RefObject, type SetStateAction } from "react";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agents/agentProfiles";
import { agentLabel, agentSupportsChat, chatAgentTypes, featuredAgentTypes, type AgentType } from "../agents/catalog";
import { leavingMenu } from "../lib/motion";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { defaultProviderProfileSelection, type Agent, type ProviderProfile } from "../state/types";
import { AgentIcon, IconCheck } from "../ui/Icons";
import type { ConfigChoice } from "../api/generated/wire";

export interface Choice extends ConfigChoice {
    icon?: ReactNode;
}

export interface AgentChoicesProps {
    type: Agent["type"];
    profile?: ProviderProfile;
    onSwitch: (type: AgentType, profileId?: string) => void;
    /** One row for each agent rather than each account, where the account is picked elsewhere. */
    perAgent?: boolean;
    /** A choice before the agents that leaves the pick to Sikemux, pressed when nothing is picked. */
    any?: { label: string; chosen: boolean; onPick: () => void };
}

/* The featured agents install their adapter on first use, so they are offered
   whether or not their CLI is found yet. The rest need their own CLI. */
/** The agents a chat that has not started yet can move to, over the model list. */
function AgentChoices({ type: current, profile, onSwitch, perAgent = false, any }: AgentChoicesProps) {
    const profiles = useStore((state) => state.providerProfiles);
    const profileSelections = useStore((state) => state.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const installed = new Set((catalog.data ?? []).filter((item) => item.available !== false).map((item) => item.type));
    const harnesses = chatAgentTypes().filter((type) => featuredAgentTypes().includes(type) || type === current || installed.has(type));
    const agentOptions = harnesses.flatMap((type) => {
        const label = agentLabel(type);
        const icon = <AgentIcon type={type} size={15} className={`agent-glyph ${type}`} />;
        const owned = profiles.filter((item) => item.provider === type);
        if (owned.length === 0) return [{ value: type, label, icon }];
        if (perAgent) {
            const account = selectedProviderProfile(type, profiles, { ...defaultProviderProfileSelection(), ...profileSelections }) ?? owned[0];
            return [{ value: type === current ? type : account.id, label, icon }];
        }
        return owned.map((item) => ({ value: item.id, label: item.name, icon }));
    });
    const builtin = defaultProviderProfileSelection()[current];
    const agentValue = any?.chosen
        ? null
        : perAgent
          ? current
          : (profile?.id ?? (builtin && agentOptions.some((option) => option.value === builtin) ? builtin : current));
    return (
        <div className="chat-picker-agents" role="group" aria-label="Agent">
            {any && (
                <button
                    type="button"
                    aria-pressed={any.chosen}
                    title={any.label}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => !any.chosen && any.onPick()}>
                    <span>{any.label}</span>
                </button>
            )}
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
                        onSwitch(type, next?.id);
                    }}>
                    {option.icon}
                    <span>{option.label}</span>
                </button>
            ))}
        </div>
    );
}

/** A composer picker's open list, searched from its own field. */
export default function PickerMenu({
    name,
    value,
    options,
    agents,
    payers,
    loading,
    compact,
    empty = "No matches",
    query,
    selected,
    trigger,
    onQuery,
    onSelected,
    onSelect,
    onClose,
}: {
    name: string;
    value: string;
    options: Choice[];
    agents?: AgentChoicesProps;
    payers?: ReactNode;
    loading: boolean;
    compact: boolean;
    /** What the list says when nothing is in it. */
    empty?: string;
    query: string;
    selected: number;
    trigger: RefObject<HTMLButtonElement | null>;
    onQuery: (query: string) => void;
    onSelected: Dispatch<SetStateAction<number>>;
    onSelect: (value: string) => void;
    onClose: () => void;
}) {
    const search = useRef<HTMLInputElement>(null);
    const listId = useId();
    const filtered = options.filter((option) =>
        [option.label, option.value, option.description].join(" ").toLowerCase().includes(query.toLowerCase()),
    );
    useEffect(() => search.current?.focus(), []);

    return (
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
                    onQuery(event.target.value);
                    onSelected(0);
                }}
                onKeyDown={(event) => {
                    if (event.key === "Escape") {
                        event.preventDefault();
                        event.stopPropagation();
                        onClose();
                    }
                    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                        event.preventDefault();
                        onSelected((index) =>
                            filtered.length ? (index + (event.key === "ArrowDown" ? 1 : filtered.length - 1)) % filtered.length : 0,
                        );
                    }
                    if (event.key === "Enter" && !event.nativeEvent.isComposing && !loading && filtered[selected]) {
                        event.preventDefault();
                        onSelect(filtered[selected].value);
                        onClose();
                    }
                }}
            />
            {agents && <AgentChoices {...agents} />}
            {payers}
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
                            onMouseEnter={() => onSelected(index)}
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => {
                                onSelect(option.value);
                                onClose();
                            }}>
                            {option.icon}
                            <span>
                                <strong>{option.label}</strong>
                                {option.description && !compact && <small>{option.description}</small>}
                            </span>
                            {option.value === value && <IconCheck size={14} />}
                        </button>
                    ))}
                {!loading && filtered.length === 0 && <div className="chat-picker-hint">{options.length === 0 ? empty : "No matches"}</div>}
            </div>
        </div>
    );
}
