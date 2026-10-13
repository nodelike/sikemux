import { useSyncExternalStore } from "react";
import { agentApi } from "../api/agents";
import type { AcpAccount } from "../api/acp";
import { invalidate } from "../state/resources";
import { hasAccounts, type AccountAgentType } from "./catalog";
import type { AgentProvider, AgentType, ProviderProfile } from "../state/types";

/* An account is a provider profile: its directory holds one sign-in. */

export type AccountProvider = AccountAgentType;

export function isAccountProvider(type: AgentType | AgentProvider | null | undefined): type is AccountProvider {
    return hasAccounts(type);
}

export function accountsOf(provider: AccountProvider, profiles: readonly ProviderProfile[]): ProviderProfile[] {
    return profiles.filter((profile) => profile.provider === provider);
}

export function acpAccount(profile: ProviderProfile): AcpAccount {
    return { id: profile.id, label: profile.name, configPath: profile.configPath };
}

/** Where a chat on `profile` goes when its account runs out of usage, in the order the person keeps their accounts. */
export function fallbackAccounts(
    profile: ProviderProfile | undefined,
    profiles: readonly ProviderProfile[],
    autoSwitch: Partial<Record<AccountProvider, boolean>>,
): AcpAccount[] {
    if (!profile || !isAccountProvider(profile.provider) || !autoSwitch[profile.provider]) return [];
    return accountsOf(profile.provider, profiles)
        .filter((other) => other.id !== profile.id)
        .map(acpAccount);
}

const sameDirectory = (a: string | null | undefined, b: string | null | undefined) => (a || null) === (b || null);

/** Reads an account's sign-in and limits again, after they changed. */
export function refreshAccount(provider: AccountProvider, configPath: string | undefined): void {
    invalidate(
        (kind, args) =>
            (kind === "agents.account" || kind === "agents.usage") &&
            args[0] === provider &&
            sameDirectory(args[2] as string | undefined, configPath),
    );
}

export type SignInState = { phase: "waiting"; url: string | null } | { phase: "failed"; message: string };

const signIns = new Map<string, SignInState>();
const listeners = new Set<() => void>();

function setSignIn(profileId: string, state: SignInState | null): void {
    if (state) signIns.set(profileId, state);
    else signIns.delete(profileId);
    for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

export function useSignIn(profileId: string): SignInState | undefined {
    return useSyncExternalStore(subscribe, () => signIns.get(profileId));
}

/**
 * Runs the provider's own sign-in for `profile`. The CLI opens the browser;
 * the page it opened is kept for when no browser came up. True once signed in.
 */
export async function signIn(profile: ProviderProfile, executablePath?: string): Promise<boolean> {
    if (!isAccountProvider(profile.provider) || signIns.get(profile.id)?.phase === "waiting") return false;
    const provider = profile.provider;
    setSignIn(profile.id, { phase: "waiting", url: null });
    const listening = new AbortController();
    void agentApi
        .onSignInPage((page) => {
            if (page.agent !== provider || !sameDirectory(page.configPath, profile.configPath)) return;
            if (signIns.get(profile.id)?.phase === "waiting") setSignIn(profile.id, { phase: "waiting", url: page.url });
        }, listening.signal)
        .catch(() => {});
    try {
        await agentApi.signIn(provider, executablePath, profile.configPath);
        setSignIn(profile.id, null);
        return true;
    } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        setSignIn(profile.id, message === "Sign-in stopped" ? null : { phase: "failed", message });
        return false;
    } finally {
        listening.abort();
        refreshAccount(provider, profile.configPath);
    }
}

export function cancelSignIn(profile: ProviderProfile): void {
    if (isAccountProvider(profile.provider)) void agentApi.cancelSignIn(profile.provider, profile.configPath).catch(() => {});
}

export function dismissSignIn(profileId: string): void {
    if (signIns.get(profileId)?.phase === "failed") setSignIn(profileId, null);
}

export async function submitSignInCode(profile: ProviderProfile, code: string): Promise<void> {
    if (isAccountProvider(profile.provider)) await agentApi.signInCode(profile.provider, profile.configPath, code);
}

export async function signOut(profile: ProviderProfile, executablePath?: string): Promise<void> {
    if (!isAccountProvider(profile.provider)) return;
    try {
        await agentApi.signOut(profile.provider, executablePath, profile.configPath);
    } finally {
        refreshAccount(profile.provider, profile.configPath);
    }
}
