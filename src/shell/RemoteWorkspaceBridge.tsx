import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { coreCall } from "../api/core";
import { remoteApi } from "../api/remote";
import { backdropPicture } from "../remote/backdrop";
import { usePaneImage } from "../lib/paneImage";
import { activeAgentId } from "../state/selectors";
import { swallow } from "../state/toast";
import { useStore } from "../state/store";
import { preferencesOf } from "../state/preferences";

/** Long enough that changing several settings publishes once. */
export const PUBLISH_DELAY_MS = 400;

/**
 * Tells the core which agent is on screen and which project is in front. While remote access is on, also the backdrop devices draw in.
 */
export function RemoteWorkspaceBridge() {
    const [enabled, setEnabled] = useState(false);
    const active = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));
    const focused = useWindowFocused();
    const frontProject = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" && !session.host ? session.id : null;
    });

    useEffect(() => {
        const controller = new AbortController();
        remoteApi
            .subscribe((status) => setEnabled(status.enabled), controller.signal)
            .then(() => remoteApi.status())
            .then((status) => {
                if (!controller.signal.aborted) setEnabled(status.enabled);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("remote access status")(error);
            });
        return () => controller.abort();
    }, []);

    const texture = useStore((s) => preferencesOf(s).paneShader);
    const paneImage = usePaneImage();
    useEffect(() => {
        if (!enabled) return;
        const timer = window.setTimeout(() => {
            const picture = texture && paneImage ? backdropPicture(paneImage) : null;
            remoteApi.publishBackdrop(texture, picture).catch(swallow("publish the pane backdrop to paired devices"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [enabled, texture, paneImage]);

    useEffect(() => {
        remoteApi.publishOnScreen(active ? [active] : [], focused).catch(swallow("tell the core which agent is on screen"));
    }, [active, focused]);

    // The core keeps a shell ready for the project in front. A project it does not know yet is refused until the person comes back to it.
    useEffect(() => {
        if (frontProject) coreCall("activateProject", { project: frontProject }).catch(() => {});
    }, [frontProject]);

    return null;
}

/** Whether the app's window is the one the person is using: a window behind another app is not looked at. */
function useWindowFocused(): boolean {
    const [focused, setFocused] = useState(true);
    useEffect(() => {
        let disposed = false;
        let unlisten: (() => void) | undefined;
        const onFocus = () => setFocused(true);
        const onBlur = () => setFocused(false);
        const watchPage = () => {
            window.addEventListener("focus", onFocus);
            window.addEventListener("blur", onBlur);
        };
        try {
            getCurrentWindow()
                .onFocusChanged(({ payload }) => setFocused(payload))
                .then((stop) => (disposed ? stop() : (unlisten = stop)))
                .catch(watchPage);
        } catch {
            watchPage();
        }
        return () => {
            disposed = true;
            unlisten?.();
            window.removeEventListener("focus", onFocus);
            window.removeEventListener("blur", onBlur);
        };
    }, []);
    return focused;
}
