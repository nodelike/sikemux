import { useEffect } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import * as cmd from "../state/commands";
import { notify, swallow } from "../state/toast";

/** Opens the sikemux:// links the app has been handed. It renders no UI. */
export function DeepLinkBridge() {
    useEffect(() => {
        const controller = new AbortController();
        const collect = async () => {
            for (const link of await invoke<string[]>("take_deep_links")) {
                if (!cmd.routeDeepLink(link)) notify("error", `couldn't open ${link}`);
            }
        };
        void (async () => {
            await getIpcTransport().subscribe("deep-link-available", () => void collect().catch(swallow("deep link")), { signal: controller.signal });
            if (controller.signal.aborted) return;
            await collect();
        })().catch((error: unknown) => {
            if (!controller.signal.aborted) swallow("deep link listener")(error);
        });
        return () => controller.abort();
    }, []);

    return null;
}
