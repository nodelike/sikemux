import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport } from "./transport";

export type DeviceAccess = "full" | "watch";

/** A device approved to reach this host's terminals and agents. */
export interface PairedDevice {
    /** The device's public key. */
    readonly id: string;
    readonly name: string;
    readonly platform: string;
    readonly access: DeviceAccess;
    readonly pairedAt: number;
    readonly lastSeen: number | null;
}

/** A phone that came with a ticket from this host's account and waits for the person to answer. */
export interface PendingDevice {
    readonly id: string;
    readonly deviceId: string;
    /** What the device calls itself. Nothing vouches for it. */
    readonly name: string;
    readonly platform: string;
    readonly fromAccount: boolean;
    /** When the phone stops waiting for an answer, in milliseconds since the epoch. */
    readonly expiresAt: number;
}

/** A phone let in before it asks, by its key. */
export interface AllowedDevice {
    readonly id: string;
    readonly name: string;
    readonly platform: string;
    readonly access: DeviceAccess;
}

/** Why the account let this host go: removed elsewhere, signed out, or the account was deleted. */
export type AccountRevokeReason = "removed" | "signed_out" | "account_deleted";

/** The host's live connection to its account, or, once signed out, why the account let it go. */
export interface AccountLink {
    readonly state: "connecting" | "live" | "offline" | "removed";
    /** Set once `state` is `removed`; null when the server no longer knew this host. */
    readonly reason: AccountRevokeReason | null;
    /** When `state` began, in milliseconds since the epoch. */
    readonly since: number;
}

/** The accounts server no longer works with this build: remote access and the account stay off until it updates. */
export interface UpdateRequired {
    readonly current: string;
    readonly minimum: string;
}

/**
 * Whether a phone's notifications from this host reach it: `phoneOff` when they are off on the
 * phone itself, `notReaching` when Apple or Google could not deliver the last one, `otherAccount`
 * when the phone is not on this host's account, `signedOut` while this host is signed out.
 */
export type NotificationState = "off" | "on" | "phoneOff" | "notReaching" | "otherAccount" | "signedOut";

/** A phone that asked this host for notifications. */
export interface PhoneNotifications {
    readonly deviceId: string;
    readonly state: NotificationState;
    /** When `state` began, in milliseconds since the epoch. */
    readonly since: number;
}

export interface RemoteStatus {
    readonly enabled: boolean;
    readonly coreId: string;
    readonly addresses: readonly string[];
    readonly devices: readonly PairedDevice[];
    readonly connected: readonly string[];
    readonly pending: readonly PendingDevice[];
    /** The account this host is signed in to. */
    readonly owner: string | null;
    readonly account: AccountLink | null;
    readonly updateRequired: UpdateRequired | null;
    readonly notifications: readonly PhoneNotifications[];
}

/** A project a paired device may start an agent in. */
export interface PublishedProject {
    readonly id: string;
    readonly name: string;
    readonly path: string;
}

/** A chat as the rail lists it, so paired devices show it under the same name. */
export interface PublishedChat {
    readonly agentId: string;
    readonly provider: string;
    readonly title: string | null;
    readonly cwd: string;
    readonly asleep: boolean;
}

/** A saved chat the rail lists as recent, which a paired device may take up again with `launcher`. */
export interface PublishedRecent {
    readonly launcher: string;
    readonly provider: string;
    readonly sessionId: string;
    readonly title: string;
    readonly cwd: string;
    /** Unix milliseconds. */
    readonly activeAt: number;
}

/** One chat agent the app offers paired devices; the app works out how to run it. */
export interface LauncherRequest {
    readonly id: string;
    readonly provider: string;
    readonly label: string;
    readonly configPath?: string;
    readonly executablePath?: string;
    readonly environmentKeys: readonly string[];
    readonly permissionMode: string;
    /** Whether the agent is ready, signed out, missing or broken, for devices to show. */
    readonly status?: string;
}

export const REMOTE_STATUS_EVENT = "remote_status_changed";

export const remoteApi = {
    status: () => invoke<RemoteStatus>("remote_status"),
    setEnabled: (enabled: boolean) => invoke<RemoteStatus>("remote_set_enabled", { enabled }),
    setDeviceAccess: (id: string, access: DeviceAccess) => invoke<RemoteStatus>("remote_set_device_access", { id, access }),
    revokeDevice: (id: string) => invoke<RemoteStatus>("remote_revoke_device", { id }),
    answerPairing: (id: string, allow: boolean, access: DeviceAccess) => invoke<RemoteStatus>("remote_answer_pairing", { id, allow, access }),
    allowDevices: (devices: readonly AllowedDevice[]) => invoke<RemoteStatus>("remote_allow_devices", { devices }),
    publishWorkspace: (projects: readonly PublishedProject[], launchers: readonly LauncherRequest[]) =>
        invoke<void>("remote_publish_workspace", { projects, launchers }),
    publishAgents: (chats: readonly PublishedChat[], titles: Readonly<Record<string, string>>) =>
        invoke<void>("remote_publish_agents", { chats, titles }),
    publishRecent: (chats: readonly PublishedRecent[]) => invoke<void>("remote_publish_recent", { chats }),
    publishOnScreen: (agentIds: readonly string[]) => invoke<void>("remote_publish_on_screen", { agentIds }),
    publishPalette: (palette: Readonly<Record<string, string>>) => invoke<void>("remote_publish_palette", { palette }),
    publishBackdrop: (texture: boolean, image: { readonly id: string; readonly dataUrl: string } | null) =>
        invoke<void>("remote_publish_backdrop", { texture, image }),
    subscribe: (listener: (status: RemoteStatus) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<RemoteStatus>(REMOTE_STATUS_EVENT, (event) => listener(event.payload), { signal }),
};

/** The first eight characters of a key, enough to tell two devices apart by eye. */
export function shortKey(key: string): string {
    return key.slice(0, 8);
}
