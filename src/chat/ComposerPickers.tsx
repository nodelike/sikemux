import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AgentIcon, IconChevron, IconUser } from "@shared/ui/Icons";
import { agentLabel, hasAccounts, type AgentType } from "@shared/agents/catalog";
import { modelProvidersApi, modelWords, type ModelProvider, type ServedModels } from "../api/modelProviders";
import { accountName, accountsOf, moveChat } from "../agents/accounts";
import type { AgentUsage } from "../api/agents";
import { accountItem } from "../rail/AgentAccounts";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import { fetchResource, peekResource } from "../state/resources";
import { agentAccountR, agentUsageR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import { defaultProviderProfileSelection, type Agent, type ProviderProfile } from "../state/types";
import { selectedProviderProfile } from "../agents/agentProfiles";
import * as cmd from "../state/commands";
import PickerMenu, { type AgentChoicesProps, type Choice } from "./PickerMenu";
import type { ConfigPicker } from "@shared/wire.generated";
import { currentLabel } from "@shared/chat/sessionConfig";

export function Picker({
    name,
    label,
    value,
    options,
    disabled,
    onSelect,
    icon,
    agents,
    payers,
    loading = false,
    onOpen,
    compact = false,
    field = false,
    empty,
}: {
    name: string;
    label: string;
    value: string;
    options: Choice[];
    disabled: boolean;
    onSelect: (value: string) => void;
    icon?: ReactNode;
    agents?: AgentChoicesProps;
    /** Who pays for the model's turns: the agent's own account or a connected provider. */
    payers?: ReactNode;
    /** Holds the menu open while the control is disabled, with the list waiting on new options. */
    loading?: boolean;
    onOpen?: () => void;
    /** Descriptions stay searchable but go unrendered, so the rows read as one line. */
    compact?: boolean;
    /** Drawn as a form field that opens downward, for settings rather than the composer. */
    field?: boolean;
    /** What the list says when it has no options. */
    empty?: string;
}) {
    const [open, setOpen] = useState(false);
    const [query, setQuery] = useState("");
    const [selected, setSelected] = useState(0);
    const root = useRef<HTMLDivElement>(null);
    const trigger = useRef<HTMLButtonElement>(null);
    const shown = open && (!disabled || loading);
    const close = () => {
        setOpen(false);
        trigger.current?.focus();
    };

    useEffect(() => {
        if (!open) return;
        const outside = (event: PointerEvent) => {
            if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
        };
        document.addEventListener("pointerdown", outside);
        return () => document.removeEventListener("pointerdown", outside);
    }, [open]);

    return (
        <div
            className={`chat-picker${field ? " field" : ""}`}
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
                <PickerMenu
                    name={name}
                    value={value}
                    options={options}
                    agents={agents}
                    payers={payers}
                    loading={loading}
                    compact={compact}
                    empty={empty}
                    query={query}
                    selected={selected}
                    trigger={trigger}
                    onQuery={setQuery}
                    onSelected={setSelected}
                    onSelect={onSelect}
                    onClose={close}
                />
            )}
        </div>
    );
}

/** A connected provider this agent can run on, with the models it serves. */
export interface Route {
    provider: ModelProvider;
    served: ServedModels;
}

/** The providers `agent` can run on, read when asked, since local runners are
    probed and keyed providers asked for their models over the network. */
export function useRoutes(type: Agent["type"], modelProvider: string | null | undefined): [Route[], () => void] {
    const [routes, setRoutes] = useState<Route[]>([]);
    const load = useCallback(() => {
        void modelProvidersApi
            .list()
            .then((providers) =>
                Promise.all(
                    providers
                        .filter((provider) => provider.connected && provider.agents.includes(type))
                        .map(async (provider) => ({
                            provider,
                            served: provider.models
                                ? { ...provider, models: provider.models }
                                : await modelProvidersApi.models(provider.id).catch(() => ({ ...provider, models: [] })),
                        })),
                ),
            )
            .then((loaded) => setRoutes(loaded.filter((route) => route.served.models.length > 0 || route.provider.id === modelProvider)))
            .catch(swallow("model providers"));
    }, [type, modelProvider]);
    useEffect(() => {
        if (modelProvider) load();
    }, [modelProvider, load]);
    return [routes, load];
}

function payerTitle(provider: ModelProvider): string {
    return provider.kind === "local" ? `Runs on ${provider.label} on this computer` : `Paid with your ${provider.label} key`;
}

export function servedLabel(route: Route, model: string): string {
    return [model, ...modelWords(route.served, route.provider.checks, model)].join(" · ");
}

/** Who pays for the chat's turns: the agent's own account, or a provider the
    person connected. Picking a provider moves the chat there, on its coding model. */
export function PayerChoices({
    type,
    modelProvider,
    profile,
    routes,
    onPick,
}: {
    type: Agent["type"];
    modelProvider: string | null | undefined;
    profile?: ProviderProfile;
    routes: Route[];
    onPick: (route?: Route) => void;
}) {
    const account = profile?.name ?? `${agentLabel(type)} account`;
    return (
        <div className="chat-picker-agents" role="group" aria-label="Paid by">
            <button
                type="button"
                aria-pressed={!modelProvider}
                title={`Paid by your ${account}`}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => modelProvider && onPick()}>
                <span>{account}</span>
            </button>
            {routes.map((route) => (
                <button
                    type="button"
                    key={route.provider.id}
                    aria-pressed={route.provider.id === modelProvider}
                    title={payerTitle(route.provider)}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => route.provider.id !== modelProvider && onPick(route)}>
                    <span>{route.provider.label}</span>
                </button>
            ))}
        </div>
    );
}

interface Movable {
    account: ProviderProfile;
    usage?: AgentUsage;
}

/** The person's other accounts with this chat's provider that can take the chat over: signed in, and once it has
    started, keeping chats where this one does. */
function useMovable(agent: Agent, profile: ProviderProfile | undefined, started: boolean): [Movable[], (fresh: boolean) => void] {
    const profiles = useStore((s) => s.providerProfiles);
    const [movable, setMovable] = useState<Movable[]>([]);
    const provider = hasAccounts(agent.type) ? agent.type : undefined;
    const accounts = useMemo(() => (provider ? accountsOf(provider, profiles) : []), [provider, profiles]);
    const load = useCallback(
        (fresh: boolean) => {
            if (!provider || !profile) return;
            const read = async (account: ProviderProfile) => {
                const status =
                    (!fresh && peekResource(agentAccountR, provider, account.configPath)) ||
                    (await fetchResource(agentAccountR, provider, account.configPath));
                const usage = status.signedIn
                    ? (!fresh && peekResource(agentUsageR, provider, account.configPath)) ||
                      (await fetchResource(agentUsageR, provider, account.configPath).catch(() => undefined))
                    : undefined;
                return { account, status, usage };
            };
            void Promise.all(accounts.map((account) => read(account).catch(() => undefined))).then((rows) => {
                const sessions = rows.find((row) => row?.account.id === profile.id)?.status.sessions;
                setMovable(
                    rows.flatMap((row) =>
                        row && row.status.signedIn && (!started || (sessions && row.status.sessions === sessions))
                            ? [{ account: row.account, usage: row.usage }]
                            : [],
                    ),
                );
            });
        },
        [provider, profile, accounts, started],
    );
    useEffect(() => load(false), [load]);
    return [movable, load];
}

/** Which of the person's accounts the chat runs on. A chat that has started moves there, even in the middle of a
    turn; one that has not simply starts there. Its menu is the rail's, without signing in or out. */
export function AccountPicker({
    agent,
    profile: chosen,
    started,
    running,
    disabled,
}: {
    agent: Agent;
    profile: ProviderProfile | undefined;
    started: boolean;
    running: boolean;
    disabled: boolean;
}) {
    const profile = useStore(
        (s) =>
            chosen ??
            selectedProviderProfile(agent.type, s.providerProfiles, { ...defaultProviderProfileSelection(), ...s.selectedProviderProfileIds }),
    );
    const [movable, load] = useMovable(agent, profile, started);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const trigger = useRef<HTMLButtonElement>(null);
    const provider = agent.type as AgentType;
    if (!profile || !movable.some((row) => row.account.id !== profile.id)) return null;
    const name = accountName(profile, peekResource(agentAccountR, provider, profile.configPath));
    const move = (account: ProviderProfile) =>
        started ? moveChat(agent, account) : cmd.configureEmptyAgent(agent.id, agent.type, account.id, agent.model, agent.modelProvider);
    const items: CtxItem[] = movable.map(({ account, usage }) => ({
        ...accountItem(provider, account, account.id === profile.id, usage),
        run: account.id === profile.id ? undefined : () => move(account),
    }));
    return (
        <div className="chat-picker">
            <button
                ref={trigger}
                type="button"
                className="chat-picker-trigger"
                aria-label="Account"
                aria-haspopup="menu"
                aria-expanded={menu !== null}
                disabled={disabled}
                title={running ? "Moving stops this turn and carries it on in the other account" : name}
                onClick={() => {
                    if (menu) return setMenu(null);
                    load(true);
                    const box = trigger.current!.getBoundingClientRect();
                    setMenu({ x: box.right, y: box.top - 6 });
                }}>
                <IconUser size={12} />
                <span>{name}</span>
                <IconChevron size={10} />
            </button>
            {menu && (
                <TreeContextMenu
                    x={menu.x}
                    y={menu.y}
                    width={280}
                    above
                    alignRight
                    className="agent-account-menu"
                    tickBesideLabel
                    items={items}
                    onClose={() => setMenu(null)}
                />
            )}
        </div>
    );
}

export function ComposerPickers({
    agent,
    profile,
    model: modelPicker,
    effort: effortPicker,
    disabled,
    agentLocked = false,
    onConfig,
    onAgent,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    /** The session's model picker and effort picker, as the core reads them. */
    model?: ConfigPicker | null;
    effort?: ConfigPicker | null;
    disabled: boolean;
    agentLocked?: boolean;
    onAgent: (type: AgentType, profileId?: string, model?: string, modelProvider?: string) => void;
    onConfig: (picker: ConfigPicker, value: string) => void;
}) {
    const slots = [{ id: "model", picker: modelPicker }, ...(effortPicker ? [{ id: effortPicker.id, picker: effortPicker }] : [])];
    const agentIcon = <AgentIcon type={agent.type} size={17} className={`agent-glyph ${agent.type}`} />;
    const rowIcon = <AgentIcon type={agent.type} size={15} className={`agent-glyph ${agent.type}`} />;
    /* The model picker an agent switch started from; a new one says the new agent's session is ready. */
    const [switching, setSwitching] = useState<{ from: ConfigPicker | null | undefined }>();
    const [routes, loadRoutes] = useRoutes(agent.type, agent.modelProvider);
    const loading = switching !== undefined && (disabled || (modelPicker != null && switching.from === modelPicker));
    useEffect(() => {
        if (switching && !loading) setSwitching(undefined);
    }, [switching, loading]);
    const route = routes.find((item) => item.provider.id === agent.modelProvider);
    const relaunch: typeof onAgent = (...choice) => {
        setSwitching({ from: modelPicker });
        onAgent(...choice);
    };
    return (
        <div className="chat-pickers">
            {slots.map(({ id, picker: config }) => {
                const model = id === "model";
                const name = model ? "Model" : "Reasoning effort";
                const current = currentLabel(config) ?? (model ? agent.model || "Model" : agent.effort || "Effort");
                const label = model && route ? `${current} · ${route.provider.label}` : current;
                const listed = config?.choices || [];
                const served = model ? (route?.served.models ?? []).filter((value) => !listed.some((option) => option.value === value)) : [];
                const options = [...listed, ...served.map((value) => ({ value, label: route ? servedLabel(route, value) : value }))];
                return (
                    <Picker
                        key={id}
                        name={name}
                        label={label}
                        value={config?.current || ""}
                        options={model ? options.map((option) => ({ ...option, icon: rowIcon })) : options}
                        disabled={disabled || (!options.length && (!model || agentLocked))}
                        icon={model ? agentIcon : undefined}
                        agents={
                            model && !agentLocked
                                ? { type: agent.type, profile, perAgent: true, onSwitch: (type, profileId) => relaunch(type, profileId) }
                                : undefined
                        }
                        payers={
                            model && !agentLocked && routes.length > 0 ? (
                                <PayerChoices
                                    type={agent.type}
                                    modelProvider={agent.modelProvider}
                                    profile={profile}
                                    routes={routes}
                                    onPick={(next) => relaunch(agent.type, profile?.id, next?.served.defaults[agent.type], next?.provider.id)}
                                />
                            ) : undefined
                        }
                        loading={model && loading}
                        onOpen={
                            model
                                ? () => {
                                      setSwitching(undefined);
                                      loadRoutes();
                                  }
                                : undefined
                        }
                        compact={model}
                        onSelect={(value) => {
                            if (config && value !== config.current) onConfig(config, value);
                        }}
                    />
                );
            })}
        </div>
    );
}
