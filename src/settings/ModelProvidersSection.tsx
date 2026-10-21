import { useCallback, useEffect, useState } from "react";
import { errorText } from "../lib/format";
import { checkWords, modelProvidersApi, modelWords, type ModelHelper, type ModelProvider, type ModelProviderKind } from "../api/modelProviders";
import { agentLabel, CHAT_AGENT_TYPES } from "../agents/catalog";
import { reportError } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";
import { openUrl } from "../api/system";

const KEYED_SECTIONS: { kind: ModelProviderKind; title: string; sub: string }[] = [
    {
        kind: "plan",
        title: "Coding plans",
        sub: "Monthly subscriptions made for coding agents. OpenCode, Pi and OMP list a plan's models once its key is saved here, and a chat's model picker can move Claude Code or Codex onto it. Keys stay in your Keychain.",
    },
    {
        kind: "api",
        title: "Model providers",
        sub: "Hosted models you pay the provider for by the token. OpenCode, Pi and OMP list a provider's models once its key is saved here, and a chat's model picker can move Claude Code or Codex onto it. Keys stay in your Keychain.",
    },
];

/** Which of Claude Code and Codex a chat can move onto the provider, in words. */
function chatsOn(provider: ModelProvider): string {
    const agents = provider.agents.map(agentLabel);
    return agents.length ? `${agents.join(" and ")} chats can run on it from the model picker.` : "";
}

/** The model a check asks about: the provider's coding model, else the first a local runner has. */
const checkedModel = (provider: ModelProvider) => provider.model ?? provider.models?.[0];

/** The model it starts on with what models.dev says of it, and each model checked so far. */
function ModelLines({ provider }: { provider: ModelProvider }) {
    const checked = Object.entries(provider.checks);
    if (!provider.model && !checked.length) return null;
    return (
        <span className="model-provider-checks">
            {provider.model && <span>Starts on {[provider.model, ...modelWords(provider, {}, provider.model)].join(" · ")}</span>}
            {checked.map(([model, check]) => (
                <span key={model} title={check.detail} className={check.works ? undefined : "model-provider-failure"}>
                    {model} {checkWords(check)}, {new Date(check.at).toLocaleDateString()}
                </span>
            ))}
        </span>
    );
}

const openSignUp = (provider: ModelProvider) => void openUrl(provider.signUpUrl).catch(reportError("Open link"));

export function ModelProvidersSection() {
    const [providers, setProviders] = useState<ModelProvider[]>([]);
    const [editing, setEditing] = useState<string | null>(null);
    const [key, setKey] = useState("");
    const [busy, setBusy] = useState<string | null>(null);
    const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
    const [checking, setChecking] = useState(false);

    const refresh = useCallback(() => modelProvidersApi.list().then(setProviders).catch(reportError("Model providers")), []);
    useEffect(() => {
        void refresh();
    }, [refresh]);

    const change = async (id: string, work: () => Promise<void>) => {
        setBusy(id);
        setFailure(null);
        try {
            await work();
            setEditing(null);
            setKey("");
            await refresh();
        } catch (error) {
            setFailure({ id, message: errorText(error) });
        } finally {
            setBusy(null);
        }
    };

    const edit = (id: string | null) => {
        setEditing(id);
        setKey("");
        setFailure(null);
    };

    const checkTools = (provider: ModelProvider, model: string) =>
        change(provider.id, async () => {
            await modelProvidersApi.check(provider.id, model);
        });

    const checkButton = (provider: ModelProvider) => {
        const model = checkedModel(provider);
        if (!model) return null;
        const isBusy = busy === provider.id;
        return (
            <button
                className="settings-btn"
                type="button"
                disabled={isBusy}
                aria-busy={isBusy || undefined}
                title={`Send ${model} one short request offering a tool`}
                onClick={() => void checkTools(provider, model)}>
                {isBusy ? "Checking…" : "Check tool calls"}
            </button>
        );
    };

    const checkAgain = async () => {
        setChecking(true);
        await refresh();
        setChecking(false);
    };

    const keyedRow = (provider: ModelProvider) => {
        const isEditing = editing === provider.id;
        const isBusy = busy === provider.id;
        const failed = failure?.id === provider.id ? failure.message : null;
        return (
            <SettingsRow
                key={provider.id}
                label={provider.label}
                wide={isEditing}
                desc={
                    failed ? (
                        <span className="model-provider-failure">{failed}</span>
                    ) : provider.connected ? (
                        <>
                            {`Connected. Agents started from now on can use it. ${chatsOn(provider)}`.trim()}
                            <ModelLines provider={provider} />
                        </>
                    ) : undefined
                }>
                {provider.connected && checkButton(provider)}
                {provider.connected ? (
                    <button
                        className="settings-btn danger"
                        type="button"
                        disabled={isBusy}
                        onClick={() => void change(provider.id, () => modelProvidersApi.disconnect(provider.id))}>
                        Disconnect
                    </button>
                ) : isEditing ? (
                    <form
                        className="model-provider-key"
                        onSubmit={(event) => {
                            event.preventDefault();
                            void change(provider.id, () => modelProvidersApi.connect(provider.id, key));
                        }}>
                        <input
                            className="settings-input mono"
                            type="password"
                            autoComplete="off"
                            spellCheck={false}
                            aria-label={`${provider.label} API key`}
                            placeholder="Paste the API key"
                            autoFocus
                            value={key}
                            onChange={(event) => setKey(event.target.value)}
                            onKeyDown={(event) => {
                                if (event.key === "Escape") edit(null);
                            }}
                        />
                        <button className="settings-btn" type="button" disabled={isBusy} onClick={() => edit(null)}>
                            Cancel
                        </button>
                        <button className="settings-btn primary" type="submit" disabled={isBusy || !key.trim()} aria-busy={isBusy || undefined}>
                            {isBusy ? "Checking…" : "Connect"}
                        </button>
                    </form>
                ) : (
                    <>
                        <button className="settings-btn" type="button" onClick={() => openSignUp(provider)}>
                            Get a key
                        </button>
                        <button className="settings-btn" type="button" onClick={() => edit(provider.id)}>
                            Add key
                        </button>
                    </>
                )}
            </SettingsRow>
        );
    };

    const localRow = (provider: ModelProvider) => (
        <SettingsRow key={provider.id} label={provider.label} desc={<RunnerState provider={provider} />}>
            {!provider.connected && (
                <button className="settings-btn" type="button" onClick={() => openSignUp(provider)}>
                    Download
                </button>
            )}
            {provider.connected && checkButton(provider)}
            <button className="settings-btn" type="button" disabled={checking} aria-busy={checking || undefined} onClick={() => void checkAgain()}>
                {checking ? "Checking…" : "Check again"}
            </button>
        </SettingsRow>
    );

    const ofKind = (kind: ModelProviderKind) => providers.filter((provider) => provider.kind === kind);
    const local = ofKind("local");
    const running = local.filter((provider) => provider.connected).length;
    return (
        <>
            {KEYED_SECTIONS.map(({ kind, title, sub }) => {
                const rows = ofKind(kind);
                const connectedCount = rows.filter((provider) => provider.connected).length;
                return (
                    <SettingsSection key={kind} title={title} meta={connectedCount ? `${connectedCount} connected` : undefined} sub={sub}>
                        <SettingsRows>{rows.map(keyedRow)}</SettingsRows>
                    </SettingsSection>
                );
            })}
            <SettingsSection
                title="Local models"
                meta={running ? `${running} running` : undefined}
                sub="Models served on this computer by Ollama or LM Studio. They need no key, and nothing leaves this computer.">
                <SettingsRows>{local.map(localRow)}</SettingsRows>
            </SettingsSection>
            <HelpersSection providers={providers} />
        </>
    );
}

function RunnerState({ provider }: { provider: ModelProvider }) {
    if (!provider.connected) return <>Nothing is answering at {new URL(provider.openai).host}.</>;
    const models = provider.models ?? [];
    if (!models.length) return <>Running, with no models downloaded yet.</>;
    return (
        <>
            Running with <span className="model-provider-models">{models.join(", ")}</span>. {chatsOn(provider)}
            <ModelLines provider={provider} />
        </>
    );
}

const SAME_AS_LEAD = "";

/** What helpers run on: agents other agents start without naming a model, and Claude Code's own subagents. */
function HelpersSection({ providers }: { providers: ModelProvider[] }) {
    const [helper, setHelper] = useState<ModelHelper | null>(null);
    const [draft, setDraft] = useState<string | null>(null);
    useEffect(() => {
        modelProvidersApi.helper().then(setHelper).catch(reportError("Helpers"));
    }, []);
    const save = (next: ModelHelper | null) =>
        modelProvidersApi
            .setHelper(next)
            .then(() => setHelper(next))
            .catch(reportError("Helpers"));
    const routes = helper ? providers.filter((provider) => provider.connected && provider.agents.includes(helper.agent)) : [];
    const route = routes.find((provider) => provider.id === helper?.provider);
    return (
        <SettingsSection
            title="Helpers"
            sub="Agents that other agents start without naming a model run as set here, and Claude Code's own subagents take this model when their chat runs where helpers do. The agent you build with keeps the model you pick.">
            <SettingsRows>
                <SettingsRow label="Agent">
                    <Dropdown
                        value={helper?.agent ?? SAME_AS_LEAD}
                        options={[
                            { value: SAME_AS_LEAD, label: "As the agent that starts them" },
                            ...CHAT_AGENT_TYPES.map((type) => ({ value: type, label: agentLabel(type) })),
                        ]}
                        onChange={(agent) => void save(agent ? { agent } : null)}
                    />
                </SettingsRow>
                {helper && (
                    <>
                        <SettingsRow label="Paid by">
                            <Dropdown
                                value={helper.provider ?? ""}
                                options={[
                                    { value: "", label: `${agentLabel(helper.agent)} account` },
                                    ...routes.map((provider) => ({ value: provider.id, label: provider.label })),
                                ]}
                                onChange={(provider) => void save({ agent: helper.agent, ...(provider ? { provider } : {}) })}
                            />
                        </SettingsRow>
                        <SettingsRow label="Model">
                            <input
                                className="settings-input mono"
                                spellCheck={false}
                                aria-label="Helper model"
                                placeholder={route?.defaults[helper.agent] ?? "The agent's own"}
                                value={draft ?? helper.model ?? ""}
                                onChange={(event) => setDraft(event.target.value)}
                                onBlur={() => {
                                    const model = draft?.trim();
                                    setDraft(null);
                                    if (model !== undefined && model !== (helper.model ?? "")) {
                                        void save({
                                            agent: helper.agent,
                                            ...(helper.provider ? { provider: helper.provider } : {}),
                                            ...(model ? { model } : {}),
                                        });
                                    }
                                }}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter") event.currentTarget.blur();
                                }}
                            />
                        </SettingsRow>
                    </>
                )}
            </SettingsRows>
        </SettingsSection>
    );
}
