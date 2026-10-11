import { create } from "zustand";
import { accountApi, type AccountPhone, type AccountStatus } from "../api/account";

export const useAccount = create<{ account: AccountStatus | null; phonesToAllow: readonly AccountPhone[] }>(() => ({
    account: null,
    phonesToAllow: [],
}));

export const setAccount = (account: AccountStatus): void => useAccount.setState({ account });

/** Right after signing in, asks the person about the phones already on the account, which would otherwise each ask later. */
export async function offerAccountPhones(): Promise<void> {
    const phonesToAllow = await accountApi.phones();
    useAccount.setState({ phonesToAllow });
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

export function initials(name: string | null, email: string | null): string {
    const words = (name ?? "").trim().split(/\s+/).filter(Boolean);
    if (words.length > 0) {
        const first = words[0] ?? "";
        const last = words.length > 1 ? (words[words.length - 1] ?? "") : "";
        return (Array.from(first)[0] ?? "").concat(Array.from(last)[0] ?? "").toLocaleUpperCase();
    }
    return (Array.from((email ?? "").trim())[0] ?? "").toLocaleUpperCase();
}
