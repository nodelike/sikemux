import { useCallback, useEffect, useState } from "react";
import { modelProvidersApi, type ModelProvider } from "../api/modelProviders";
import { portsApi } from "../api/ports";
import { invalidate } from "../state/resources";
import { reportError } from "../state/toast";
import { SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

export function ModelProvidersSection() {
    const [providers, setProviders] = useState<ModelProvider[]>([]);
    const [editing, setEditing] = useState<string | null>(null);
    const [key, setKey] = useState("");
    const [busy, setBusy] = useState<string | null>(null);
    const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);

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
            invalidate((kind) => kind === "agents.models");
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

    const connectedCount = providers.filter((provider) => provider.connected).length;
    return (
        <SettingsSection
            title="Model providers"
            meta={connectedCount ? `${connectedCount} connected` : undefined}
            sub="Hosted models you pay the provider for. OpenCode, Pi and OMP list a provider's models once its key is saved here. Keys stay in your Keychain.">
            <SettingsRows>
                {providers.map((provider) => {
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
                                    "Connected. Agents started from now on can use it."
                                ) : undefined
                            }>
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
                                    <button
                                        className="settings-btn primary"
                                        type="submit"
                                        disabled={isBusy || !key.trim()}
                                        aria-busy={isBusy || undefined}>
                                        {isBusy ? "Checking…" : "Connect"}
                                    </button>
                                </form>
                            ) : (
                                <>
                                    <button
                                        className="settings-btn"
                                        type="button"
                                        onClick={() => void portsApi.openExternal(provider.keysUrl).catch(reportError("Open link"))}>
                                        Get a key
                                    </button>
                                    <button className="settings-btn" type="button" onClick={() => edit(provider.id)}>
                                        Add key
                                    </button>
                                </>
                            )}
                        </SettingsRow>
                    );
                })}
            </SettingsRows>
        </SettingsSection>
    );
}
