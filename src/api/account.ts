import { coreCall } from "./core";
import type { CoreChange } from "./coreChanged";
import type { AccountInfo } from "@shared/wire.generated";
import { openUrl } from "./system";
import { getIpcTransport } from "./transport";

/** The Sikemux account this host is signed in to, which devices on the same account find it through. */
export interface AccountStatus {
    readonly signedIn: boolean;
    readonly email: string | null;
    readonly name: string | null;
    /** The account's picture as a `data:` URL, which the core keeps so it shows offline. */
    readonly picture: string | null;
}

const SIGNED_OUT: AccountStatus = { signedIn: false, email: null, name: null, picture: null };

/** The picture last read, by the hash the core names it with. */
let picture: { hash: string; url: string | null } | null = null;

async function pictureOf(hash: string | null): Promise<string | null> {
    if (!hash) return null;
    if (picture?.hash !== hash) {
        const read = await coreCall<{ mime: string; data: string } | null>("accountPicture", { hash });
        picture = { hash, url: read && `data:${read.mime};base64,${read.data}` };
    }
    return picture.url;
}

async function statusOf(info: AccountInfo | null): Promise<AccountStatus> {
    if (!info) return SIGNED_OUT;
    return { signedIn: true, email: info.email, name: info.name, picture: await pictureOf(info.pictureHash) };
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

export const accountApi = {
    /** Takes a device or host off the account, which revokes it on every host at once. */
    removeDevice: (key: string) => coreCall<void>("removeAccountDevice", { key }),
    /** Renames a device or host on the account, which every device and host shows from then on. */
    renameDevice: (key: string, name: string) => coreCall<void>("renameAccountDevice", { key, name }),
    /** Who the core says this host is signed in to Sikemux as. */
    status: async () => statusOf(await coreCall<AccountInfo | null>("account")),
    /** Opens sign-in in the browser and resolves once the core has registered this host with the account. */
    signIn: async () => {
        const { url } = await coreCall<{ url: string }>("accountSignIn");
        await openUrl(url);
        return statusOf(await coreCall<AccountInfo>("accountSignInFinish"));
    },
    cancelSignIn: () => coreCall<void>("accountSignInCancel"),
    /** The phones on the account this host has not paired yet. */
    phones: () => coreCall<readonly AccountPhone[]>("accountPhones"),
    /** The other hosts on the account this host has not linked with yet. */
    /** Asks another host on the account to link, and resolves once the person there allows it. */
    /** Takes this host off the account; devices already paired stay paired. */
    signOut: async () => {
        await coreCall<void>("accountSignOut");
        return SIGNED_OUT;
    },
    /** Follows the account when it changes, such as this host being removed from it elsewhere. */
    subscribe: (listener: (status: AccountStatus) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<CoreChange>(
            "core_changed",
            ({ payload }) => {
                if (payload.topic === "account") void accountApi.status().then(listener);
            },
            { signal },
        ),
};
