import { useMemo, useState } from "react";
import { AccountSignIn } from "../agents/AccountSignIn";
import { accountName, accountsOf, openAccountDialog, signOut, switchAccount, useSignIn } from "../agents/accounts";
import type { AgentType } from "../agents/catalog";
import { selectedProviderProfile } from "../agents/agentProfiles";
import { accountAgentTypes, agentLabel } from "../agents/catalog";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { agentAccountR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import type { ProviderProfile } from "../state/types";
import { Switch } from "../ui/Controls";
import { IconLogOut, IconMore, IconPlus, IconTrash } from "../ui/Icons";
import { planLabel } from "../rail/usageFormat";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import { SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

/** Where the person keeps their Claude and Codex accounts: who each is signed in as, and everything done to them. */
export function AccountsSections() {
    return (
        <>
            {accountAgentTypes().map((type) => (
                <ProviderAccounts key={type} provider={type} label={agentLabel(type)} />
            ))}
        </>
    );
}

export function accountsSectionTitle(provider: AgentType): string {
    return `${agentLabel(provider)} accounts`;
}

function ProviderAccounts({ provider, label }: { provider: AgentType; label: string }) {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const autoSwitch = useStore((s) => s.accountAutoSwitch[provider] === true);
    const accounts = useMemo(() => accountsOf(provider, profiles), [provider, profiles]);
    const current = selectedProviderProfile(provider, profiles, selections) ?? accounts[0];

    return (
        <SettingsSection title={accountsSectionTitle(provider)} meta={accounts.length === 1 ? "1 account" : `${accounts.length} accounts`}>
            <SettingsRows>
                {accounts.map((account) => (
                    <AccountRow key={account.id} provider={provider} account={account} inUse={account.id === current?.id} />
                ))}
                {accounts.length > 1 && (
                    <SettingsRow
                        label="Auto-rotate at limits"
                        desc={`Check ${label === "Claude" ? "Anthropic's" : "OpenAI's"} terms first.`}
                        asLabel
                        control={
                            <Switch
                                checked={autoSwitch}
                                onChange={(on) => cmd.setAccountAutoSwitch(provider, on)}
                                label={`Auto-rotate ${label} accounts at limits`}
                            />
                        }
                    />
                )}
            </SettingsRows>
            <button type="button" className="account-add" onClick={() => openAccountDialog({ provider })}>
                <IconPlus size={13} />
                Add account
            </button>
        </SettingsSection>
    );
}

function AccountRow({ provider, account, inUse }: { provider: AgentType; account: ProviderProfile; inUse: boolean }) {
    const status = useResource(agentAccountR, provider, account.configPath);
    const signing = useSignIn(account.id);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const signedIn = status.data?.signedIn === true;
    const known = status.data !== undefined;
    const plan = status.data?.plan ? planLabel(status.data.plan) : null;
    const name = accountName(account, status.data);
    const who = !known
        ? status.status === "error"
            ? "Could not ask who is signed in"
            : "Checking…"
        : signedIn
          ? [status.data?.email ?? status.data?.organization, plan].filter(Boolean).join(" · ") || "Signed in"
          : "Signed out";
    const removable = !account.id.startsWith("builtin-");
    const items: CtxItem[] = [
        ...(signedIn ? [{ label: "Sign out", icon: <IconLogOut size={13} />, run: () => void signOut(account).catch(reportError("Sign out")) }] : []),
        ...(removable
            ? [{ label: `Remove ${name}`, icon: <IconTrash size={13} />, danger: true, run: () => cmd.deleteProviderProfile(account.id) }]
            : []),
    ];

    return (
        <SettingsRow
            icon={
                <span className="account-row-avatar" style={{ background: account.accent }} aria-hidden="true">
                    {name.trim().charAt(0).toUpperCase() || "?"}
                </span>
            }
            label={
                <span className="account-row-label">
                    {name}
                    {inUse && <span className="account-row-badge">In use</span>}
                </span>
            }
            desc={
                <span className={known && !signedIn ? "account-row-out" : undefined} title={account.configPath ?? undefined}>
                    {who}
                </span>
            }>
            {signing ? (
                <AccountSignIn profile={account} buttonClass="settings-btn" />
            ) : known && !signedIn ? (
                <button className="settings-btn" type="button" onClick={() => openAccountDialog({ provider, profileId: account.id })}>
                    Sign in
                </button>
            ) : (
                !inUse && (
                    <button
                        className="settings-btn"
                        type="button"
                        onClick={() => void switchAccount(provider, account).catch(reportError("Switch account"))}>
                        Use
                    </button>
                )
            )}
            {items.length > 0 && (
                <button
                    className="account-row-more"
                    type="button"
                    aria-label={`More for ${name}`}
                    aria-haspopup="menu"
                    onClick={(event) => {
                        const box = event.currentTarget.getBoundingClientRect();
                        setMenu({ x: box.right, y: box.bottom + 4 });
                    }}>
                    <IconMore size={14} />
                </button>
            )}
            {menu && <TreeContextMenu x={menu.x} y={menu.y} alignRight items={items} onClose={() => setMenu(null)} />}
        </SettingsRow>
    );
}
