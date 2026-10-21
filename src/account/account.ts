import { create } from "zustand";
import { accountApi, type AccountPhone, type AccountStatus } from "../api/account";
import * as cmd from "../state/commands";
import { reportError, swallow } from "../state/toast";

export const useAccount = create<{ account: AccountStatus | null; phonesToAllow: readonly AccountPhone[]; signingIn: boolean }>(() => ({
    account: null,
    phonesToAllow: [],
    signingIn: false,
}));

export const setAccount = (account: AccountStatus): void => useAccount.setState({ account });

/** Right after signing in, asks the person about the phones already on the account, which would otherwise each ask later. */
export async function offerAccountPhones(): Promise<void> {
    const phonesToAllow = await accountApi.phones();
    useAccount.setState({ phonesToAllow });
}

/** Signs in through the browser, then shows the account and asks about the phones already on it. */
export async function signInToAccount(): Promise<void> {
    useAccount.setState({ signingIn: true });
    try {
        setAccount(await accountApi.signIn());
        cmd.openSettings("devices");
        offerAccountPhones().catch(swallow("phones on the account"));
    } catch (error) {
        if (!String(error).includes("cancelled")) reportError("Sign in")(error);
    } finally {
        useAccount.setState({ signingIn: false });
    }
}

export async function signOutOfAccount(): Promise<void> {
    try {
        setAccount(await accountApi.signOut());
    } catch (error) {
        reportError("Sign out")(error);
    }
}

export const doneWithAccountPhones = (): void => useAccount.setState({ phonesToAllow: [] });

/** Shows what is cached first, then asks for a newer name and picture if the cached ones are old. */
export async function loadAccount(): Promise<void> {
    const cached = await accountApi.status();
    setAccount(cached);
    if (cached.signedIn) setAccount(await accountApi.refreshProfile());
}

/** Keeps the account current when it changes without the app asking. */
export async function watchAccount(signal: AbortSignal): Promise<void> {
    await accountApi.subscribe(setAccount, signal);
}
