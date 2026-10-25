import { useEffect } from "react";
import type { NetworkChange } from "@shared/wire.generated";
import { getIpcTransport } from "../api/transport";
import { refetchFailed } from "../state/resources";
import { swallow } from "../state/toast";

export const NETWORK_CHANGED_EVENT = "network_changed";

/** When the network comes back, changes or the computer wakes, whatever failed for want of it on screen is fetched again. */
export function NetworkBridge() {
    useEffect(() => {
        const controller = new AbortController();
        getIpcTransport()
            .subscribe<NetworkChange>(
                NETWORK_CHANGED_EVENT,
                ({ payload }) => {
                    if (payload !== "offline") refetchFailed();
                },
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("network listener")(error);
            });
        return () => controller.abort();
    }, []);
    return null;
}
