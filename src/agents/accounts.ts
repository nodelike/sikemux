import { useSyncExternalStore } from "react";
import { agentApi, type AgentAccountStatus } from "../api/agents";
import { acpApi, type AcpAccount } from "../api/acp";
import { invalidate } from "../state/resources";
import { reportError } from "../state/toast";
import { hasAccounts, type AccountAgentType } from "./catalog";
import { adoptAgentAccounts } from "../state/persist";
import { DEFAULT_PROVIDER_PROFILES, type Agent, type AgentProvider, type AgentType, type ProviderProfile } from "../state/types";
import { errorText } from "../lib/format";

/* An account is a provider profile: its directory holds one sign-in. */

export type AccountProvider = AccountAgentType;

export function isAccountProvider(type: AgentType | AgentProvider | null | undefined): type is AccountProvider {
    return hasAccounts(type);
}

export function accountsOf(provider: AccountProvider, profiles: readonly ProviderProfile[]): ProviderProfile[] {
    return profiles.filter((profile) => profile.provider === provider);
}

/* A profile still under its built-in name says nothing about whose account
   it is, so it goes by the name on the account instead. */
export function accountName(account: ProviderProfile, status: AgentAccountStatus | undefined): string {
    const unnamed = DEFAULT_PROVIDER_PROFILES.some((builtin) => builtin.id === account.id && builtin.name === account.name);
    return (unnamed && status?.name) || account.name;
}

export function acpAccount(profile: ProviderProfile): AcpAccount {
    return { id: profile.id, label: profile.name, configPath: profile.configPath };
}

/** Carries the chat on to `account`. A turn in progress stops and carries on there. */
export function moveChat(agent: Agent, account: ProviderProfile): void {
    void acpApi.switchAccount(agent.id, agent.type, acpAccount(account)).catch(reportError("Switch account"));
}

/** Makes `account` the one `provider` runs on: new chats start there, and the core moves open ones there with any
    turn carrying on. Chats kept where the account cannot see them, and agents in a terminal, stay where they are. */
export async function switchAccount(provider: AccountProvider, account: ProviderProfile): Promise<void> {
    adoptAgentAccounts(await agentApi.selectAccount(provider, account.id));
}

const sameDirectory = (a: string | null | undefined, b: string | null | undefined) => (a || null) === (b || null);

/** Reads an account's sign-in and limits again, after they changed. */
async function refreshAccount(provider: AccountProvider, configPath: string | undefined): Promise<void> {
    await agentApi.forgetAccount(provider, configPath).catch(() => {});
    invalidate(
        (kind, args) =>
            (kind === "agents.account" || kind === "agents.usage") &&
            args[0] === provider &&
            sameDirectory(args[1] as string | undefined, configPath),
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
        const message = errorText(error);
        setSignIn(profile.id, message === "Sign-in stopped" ? null : { phase: "failed", message });
        return false;
    } finally {
        listening.abort();
        void refreshAccount(provider, profile.configPath);
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
        void refreshAccount(profile.provider, profile.configPath);
    }
}

/** Makes a new account a folder of its own that shares the default one's chats and settings. */
export async function addAccount(provider: AccountProvider, name: string, accent: string): Promise<ProviderProfile> {
    const configPath = await agentApi.addAccount(provider, name);
    const profile: ProviderProfile = {
        id: `profile-${Date.now().toString(36)}`,
        name,
        provider,
        accent,
        configPath,
        environmentKeys: [],
    };
    adoptAgentAccounts(await agentApi.saveAccount(profile));
    return profile;
}

/** The sign-in dialog: a new account for `provider`, or `profileId` signing in again. */
export interface AccountDialogRequest {
    provider: AccountProvider;
    profileId?: string;
}

let dialog: (AccountDialogRequest & { id: number }) | null = null;
let dialogCount = 0;
const dialogListeners = new Set<() => void>();

function setDialog(next: typeof dialog): void {
    dialog = next;
    for (const listener of dialogListeners) listener();
}

export function openAccountDialog(request: AccountDialogRequest): void {
    setDialog({ ...request, id: ++dialogCount });
}

export function closeAccountDialog(): void {
    setDialog(null);
}

export function useAccountDialog() {
    return useSyncExternalStore(
        (listener) => {
            dialogListeners.add(listener);
            return () => dialogListeners.delete(listener);
        },
        () => dialog,
    );
}
