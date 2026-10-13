import { useMemo, useState } from "react";
import { agentApi } from "../api/agents";
import { AccountSignIn } from "../agents/AccountSignIn";
import { accountsOf, signIn, signOut, useSignIn, type AccountProvider } from "../agents/accounts";
import { providerAccent, selectedProviderProfile } from "../agents/agentProfiles";
import { ACCOUNT_AGENT_TYPES, agentLabel } from "../agents/catalog";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { agentAccountR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import type { ProviderProfile } from "../state/types";
import { Switch } from "../ui/Controls";
import { IconTrash } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { planLabel } from "../rail/usageFormat";
import { SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const PROVIDERS = ACCOUNT_AGENT_TYPES.map((id) => ({ id, label: agentLabel(id), accent: providerAccent(id) }));

/** Where the person keeps their Claude and Codex accounts: who each is signed in as, and everything done to them. */
export function AccountsSections() {
    return (
        <>
            {PROVIDERS.map((provider) => (
                <ProviderAccounts key={provider.id} provider={provider.id} label={provider.label} accent={provider.accent} />
            ))}
        </>
    );
}

export function accountsSectionTitle(provider: AccountProvider): string {
    return `${agentLabel(provider)} accounts`;
}

function ProviderAccounts({ provider, label, accent }: { provider: AccountProvider; label: string; accent: string }) {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const autoSwitch = useStore((s) => s.accountAutoSwitch[provider] === true);
    const accounts = useMemo(() => accountsOf(provider, profiles), [provider, profiles]);
    const current = selectedProviderProfile(provider, profiles, selections) ?? accounts[0];
    const [adding, setAdding] = useState(false);

    return (
        <SettingsSection
            title={accountsSectionTitle(provider)}
            meta={accounts.length === 1 ? "1 account" : `${accounts.length} accounts`}
            sub={`Each ${label} account signs in through ${label}'s own login and keeps the sign-in in its own folder. Chats and settings are shared, so a chat can move between them.`}>
            <SettingsRows>
                {accounts.map((account) => (
                    <AccountRow key={account.id} provider={provider} account={account} inUse={account.id === current?.id} />
                ))}
                {adding ? (
                    <AddAccountRow provider={provider} label={label} accent={accent} onDone={() => setAdding(false)} />
                ) : (
                    <SettingsRow label="Another account" desc={`Sign in to a second ${label} account, such as one for work.`}>
                        <button className="settings-btn" type="button" onClick={() => setAdding(true)}>
                            Add account
                        </button>
                    </SettingsRow>
                )}
                {accounts.length > 1 && (
                    <SettingsRow
                        label="Move chats when one runs out"
                        desc={`A chat that hits a usage limit carries on with your next signed-in ${label} account. Off by default: ${label === "Claude" ? "Anthropic's" : "OpenAI's"} terms expect one person's ordinary use per plan.`}
                        asLabel
                        control={
                            <Switch
                                checked={autoSwitch}
                                onChange={(on) => cmd.setAccountAutoSwitch(provider, on)}
                                label={`Move ${label} chats to another account at a usage limit`}
                            />
                        }
                    />
                )}
            </SettingsRows>
        </SettingsSection>
    );
}

function AccountRow({ provider, account, inUse }: { provider: AccountProvider; account: ProviderProfile; inUse: boolean }) {
    const status = useResource(agentAccountR, provider, account.executablePath, account.configPath);
    const signing = useSignIn(account.id);
    const signedIn = status.data?.signedIn === true;
    const known = status.data !== undefined;
    const plan = status.data?.plan ? planLabel(status.data.plan) : null;
    const who = !known
        ? status.status === "error"
            ? "Could not ask who is signed in"
            : "Checking…"
        : signedIn
          ? [status.data?.email ?? status.data?.organization, plan].filter(Boolean).join(" · ") || "Signed in"
          : "Signed out";

    return (
        <SettingsRow
            stack
            label={
                <span className="account-row-label">
                    <span className="account-row-avatar" style={{ background: account.accent }} aria-hidden="true">
                        {account.name.trim().charAt(0).toUpperCase() || "?"}
                    </span>
                    {account.name}
                    {inUse && <span className="account-row-badge">New chats</span>}
                </span>
            }
            desc={
                <span className={known && !signedIn ? "account-row-out" : undefined} title={account.configPath ?? undefined}>
                    {who}
                </span>
            }>
            {signing || (known && !signedIn) ? (
                <AccountSignIn profile={account} buttonClass="settings-btn" />
            ) : (
                <>
                    {!inUse && (
                        <button className="settings-btn" type="button" onClick={() => cmd.selectProviderProfile(provider, account.id)}>
                            Use for new chats
                        </button>
                    )}
                    {signedIn && (
                        <button className="settings-btn" type="button" onClick={() => void signOut(account).catch(reportError("Sign out"))}>
                            Sign out
                        </button>
                    )}
                </>
            )}
            {!account.id.startsWith("builtin-") && (
                <Tooltip label={`Remove ${account.name} from Sikemux`}>
                    <button
                        className="settings-btn danger account-row-remove"
                        type="button"
                        aria-label={`Remove ${account.name}`}
                        onClick={() => cmd.deleteProviderProfile(account.id)}>
                        <IconTrash size={12} />
                    </button>
                </Tooltip>
            )}
        </SettingsRow>
    );
}

/* A new account gets a folder of its own that shares the default one's chats
   and settings, then signs in there straight away. */
function AddAccountRow({ provider, label, accent, onDone }: { provider: AccountProvider; label: string; accent: string; onDone: () => void }) {
    const [name, setName] = useState("");
    const [busy, setBusy] = useState(false);
    const add = async () => {
        const trimmed = name.trim();
        if (!trimmed || busy) return;
        setBusy(true);
        try {
            const configPath = await agentApi.addAccount(provider, trimmed);
            const profile: ProviderProfile = {
                id: `profile-${Date.now().toString(36)}`,
                name: trimmed,
                provider,
                accent,
                configPath,
                environmentKeys: [],
            };
            cmd.saveProviderProfile(profile);
            onDone();
            void signIn(profile);
        } catch (error) {
            reportError("Add account")(error);
            setBusy(false);
        }
    };
    return (
        <SettingsRow label="New account" desc="A name for it, then sign in in your browser." wide>
            <form
                className="model-provider-key"
                onSubmit={(event) => {
                    event.preventDefault();
                    void add();
                }}>
                <input
                    className="settings-input"
                    autoFocus
                    aria-label={`New ${label} account name`}
                    placeholder="Work"
                    maxLength={40}
                    value={name}
                    disabled={busy}
                    onChange={(event) => setName(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") onDone();
                    }}
                />
                <button className="settings-btn" type="button" disabled={busy} onClick={onDone}>
                    Cancel
                </button>
                <button className="settings-btn primary" type="submit" disabled={busy || !name.trim()}>
                    Add and sign in
                </button>
            </form>
        </SettingsRow>
    );
}
