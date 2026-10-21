import { coreCall, coreRequest, type CoreRequest } from "./core";
import { invokeCommand as invoke } from "./invoke";
import type { PublishedBuddies } from "../buddies/published";
import { getIpcTransport } from "./transport";

import type { AllowedDevice, DeviceAccess, RemotePairing, RemoteStatus } from "./generated/wire";

export type { AccountLink, DeviceInfo, LinkedHost, NotificationState, PendingDevice, UpdateRequired } from "./generated/wire";
export type { AllowedDevice, DeviceAccess, RemotePairing, RemoteStatus };

export const REMOTE_STATUS_EVENT = "remote_status_changed";

/** Every change to remote access answers with where it stands now. */
function remote(request: CoreRequest): Promise<RemoteStatus> {
    return coreRequest<{ status: RemoteStatus }>(request).then(({ status }) => status);
}

/** The core keeps what was published last of each kind and the app sends it again to a core it reconnects to. */
function publish(request: { op: string } & Record<string, unknown>): Promise<void> {
    return invoke<void>("remote_publish", { request });
}

export const remoteApi = {
    status: () => remote({ op: "remoteStatus" }),
    pairing: () => coreCall<RemotePairing>("pairing"),
    setEnabled: (enabled: boolean) => remote({ op: "setRemoteAccess", enabled }),
    setDeviceAccess: (id: string, access: DeviceAccess) => remote({ op: "setDeviceAccess", id, access }),
    revokeDevice: (id: string) => remote({ op: "revokeDevice", id }),
    /** Whether each new device and host waits for the person here, rather than coming in on its account's word. */
    setRequireApproval: (requireApproval: boolean) => coreCall<RemoteStatus>("setRequireApproval", { requireApproval }),
    answerPairing: (id: string, allow: boolean, access: DeviceAccess) => remote({ op: "answerPairing", id, allow, access }),
    allowDevices: (devices: readonly AllowedDevice[]) => remote({ op: "allowDevices", devices }),
    publishOnScreen: (agentIds: readonly string[], focused: boolean) => publish({ op: "publishOnScreen", agentIds, focused }),
    publishPalette: (palette: Readonly<Record<string, string>>) => publish({ op: "publishPalette", palette }),
    publishBuddies: (buddies: PublishedBuddies) => publish({ op: "call", method: "publishBuddies", params: buddies }),
    publishBackdrop: (texture: boolean, image: { readonly id: string; readonly dataUrl: string } | null) =>
        publish({ op: "publishBackdrop", texture, image }),
    subscribe: (listener: (status: RemoteStatus) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<RemoteStatus>(REMOTE_STATUS_EVENT, (event) => listener(event.payload), { signal }),
};

/** The first eight characters of a key, enough to tell two devices apart by eye. */
export function shortKey(key: string): string {
    return key.slice(0, 8);
}
