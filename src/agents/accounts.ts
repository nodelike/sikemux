import { externalValue, useExternalValue } from "../lib/externalValue";
import { agentApi, type AgentAccountStatus } from "../api/agents";
import { acpApi, type AcpAccount } from "../api/acp";
import { invalidate } from "../state/resources";
import { reportError, swallow } from "../state/toast";
import { hasAccounts, type AgentType } from "./catalog";
import { adoptAgentAccounts } from "../state/persist";
import { defaultProviderProfiles, type Agent, type ProviderProfile } from "../state/types";
import { errorText } from "../lib/format";

/* An account is a provider profile: its directory holds one sign-in. */

export function accountsOf(provider: AgentType, profiles: readonly ProviderProfile[]): ProviderProfile[] {
    return profiles.filter((profile) => profile.provider === provider);
}

/* A profile still under its built-in name says nothing about whose account
   it is, so it goes by the name on the account instead. */
export function accountName(account: ProviderProfile, status: AgentAccountStatus | undefined): string {
    const unnamed = defaultProviderProfiles().some((builtin) => builtin.id === account.id && builtin.name === account.name);
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
export async function switchAccount(provider: AgentType, account: ProviderProfile): Promise<void> {
    adoptAgentAccounts(await agentApi.selectAccount(provider, account.id));
}

const sameDirectory = (a: string | null | undefined, b: string | null | undefined) => (a || null) === (b || null);

/** Reads an account's sign-in and limits again, after they changed. */
async function refreshAccount(provider: AgentType, configPath: string | undefined): Promise<void> {
    await agentApi.forgetAccount(provider, configPath).catch(swallow("forget the account cache"));
    invalidate(
        (kind, args) =>
            (kind === "agents.account" || kind === "agents.usage") &&
            args[0] === provider &&
            sameDirectory(args[1] as string | undefined, configPath),
    );
}

export type SignInState = { phase: "waiting"; url: string | null } | { phase: "failed"; message: string };

const signIns = externalValue<Readonly<Record<string, SignInState>>>({});

function setSignIn(profileId: string, state: SignInState | null): void {
    const { [profileId]: _was, ...rest } = signIns.get();
    signIns.set(state ? { ...rest, [profileId]: state } : rest);
}

export function useSignIn(profileId: string): SignInState | undefined {
    return useExternalValue(signIns, (all) => all[profileId]);
}

/**
 * Runs the provider's own sign-in for `profile`. The CLI opens the browser;
 * the page it opened is kept for when no browser came up. True once signed in.
 */
export async function signIn(profile: ProviderProfile, executablePath?: string): Promise<boolean> {
    if (!hasAccounts(profile.provider) || signIns.get()[profile.id]?.phase === "waiting") return false;
    const provider = profile.provider;
    setSignIn(profile.id, { phase: "waiting", url: null });
    const listening = new AbortController();
    void agentApi
        .onSignInPage((page) => {
            if (page.agent !== provider || !sameDirectory(page.configPath, profile.configPath)) return;
            if (signIns.get()[profile.id]?.phase === "waiting") setSignIn(profile.id, { phase: "waiting", url: page.url });
        }, listening.signal)
        .catch(swallow("follow sign-in pages"));
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
    if (hasAccounts(profile.provider)) void agentApi.cancelSignIn(profile.provider, profile.configPath).catch(swallow("cancel the sign-in"));
}

export function dismissSignIn(profileId: string): void {
    if (signIns.get()[profileId]?.phase === "failed") setSignIn(profileId, null);
}

export async function submitSignInCode(profile: ProviderProfile, code: string): Promise<void> {
    if (hasAccounts(profile.provider)) await agentApi.signInCode(profile.provider, profile.configPath, code);
}

export async function signOut(profile: ProviderProfile, executablePath?: string): Promise<void> {
    if (!hasAccounts(profile.provider)) return;
    try {
        await agentApi.signOut(profile.provider, executablePath, profile.configPath);
    } finally {
        void refreshAccount(profile.provider, profile.configPath);
    }
}

/** Makes a new account a folder of its own that shares the default one's chats and settings. */
export async function addAccount(provider: AgentType, name: string, accent: string): Promise<ProviderProfile> {
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
    provider: AgentType;
    profileId?: string;
}

const dialog = externalValue<(AccountDialogRequest & { id: number }) | null>(null);
let dialogCount = 0;

export function openAccountDialog(request: AccountDialogRequest): void {
    dialog.set({ ...request, id: ++dialogCount });
}

export function closeAccountDialog(): void {
    dialog.set(null);
}

export const useAccountDialog = () => useExternalValue(dialog);
