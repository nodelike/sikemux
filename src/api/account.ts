import { invokeCommand as invoke } from "./invoke";
import type { RemotePairing } from "./remote";
import { getIpcTransport } from "./transport";

/** The Sikemux account this host is signed in to, which devices on the same account find it through. */
export interface AccountStatus {
    readonly signedIn: boolean;
    readonly userId: string | null;
    readonly email: string | null;
    readonly name: string | null;
    /** The account's picture as a `data:` URL, cached on disk so it shows offline. */
    readonly picture: string | null;
}

/** A phone on the account that this host has not paired. */
export interface AccountPhone {
    /** The phone's public key, which this host pairs it by. */
    readonly key: string;
    readonly name: string;
    readonly platform: string;
    readonly createdAt: string;
}

/** Another host on the account, which this host links with by its key. */
export type AccountHost = AccountPhone;

/** Sent when the account changes without the app asking, such as this host being removed from it elsewhere. */
export const ACCOUNT_CHANGED_EVENT = "account_changed";

export const accountApi = {
    /** What this host knows without asking the network. */
    status: () => invoke<AccountStatus>("account_status"),
    /** Fetches the name and picture again once the cached ones are a few hours old. */
    refreshProfile: () => invoke<AccountStatus>("account_refresh_profile"),
    /** Opens sign-in in the browser and resolves once this host is registered with the account. */
    signIn: () => invoke<AccountStatus>("account_sign_in"),
    cancelSignIn: () => invoke<void>("account_cancel_sign_in"),
    /** The phones on the account this host has not paired yet. */
    phones: () => invoke<readonly AccountPhone[]>("account_phones"),
    /** The other hosts on the account this host has not linked with yet. */
    hosts: () => invoke<readonly AccountHost[]>("account_hosts"),
    /** Asks another host on the account to link, and resolves once the person there allows it. */
    linkHost: (host: AccountHost) => invoke<RemotePairing>("account_link_host", { key: host.key, name: host.name, platform: host.platform }),
    /** Takes this host off the account; devices already paired stay paired. */
    signOut: () => invoke<AccountStatus>("account_sign_out"),
    subscribe: (listener: (status: AccountStatus) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<AccountStatus>(ACCOUNT_CHANGED_EVENT, (event) => listener(event.payload), { signal }),
};
