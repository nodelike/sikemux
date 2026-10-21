import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useState } from "react";
import { publishBuddies, type PublishedBuddies } from "../buddies/published";
import { remoteApi } from "../api/remote";
import { backdropPicture, grainDotColor } from "../remote/backdrop";
import { usePaneImage } from "../lib/paneImage";
import { readPalette } from "../remote/palette";
import { activeAgentId } from "../state/selectors";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { swallow } from "../state/toast";
import { useStore } from "../state/store";

/** Long enough that changing several settings publishes once. */
export const PUBLISH_DELAY_MS = 400;

/**
 * Tells the core the buddies the app draws for the notch and paired devices, and which agent is on screen. While remote access is on, also the theme and backdrop devices draw in.
 */
export function RemoteWorkspaceBridge() {
    const [enabled, setEnabled] = useState(false);
    const [themeChanges, setThemeChanges] = useState(0);
    const agents = useStore((s) => s.agents);
    const buddiesOn = useStore((s) => s.buddies);
    const buddyPack = useStore((s) => s.buddyPack);
    const buddyShape = useStore((s) => s.buddyShape);
    const buddyMotion = useStore((s) => s.buddyMotion);
    const buddies = useMemo(
        () => JSON.stringify(publishBuddies(Object.values(agents), { pack: buddyPack, shape: buddyShape }, buddyMotion, buddiesOn)),
        [buddiesOn, agents, buddyPack, buddyShape, buddyMotion],
    );
    const active = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));
    const focused = useWindowFocused();

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

    const texture = useStore((s) => s.paneShader);
    const paneImage = usePaneImage();
    useEffect(() => {
        if (!enabled) return;
        const timer = window.setTimeout(() => {
            const picture = texture && paneImage ? backdropPicture(paneImage) : null;
            remoteApi.publishBackdrop(texture, picture).catch(swallow("publish the pane backdrop to paired devices"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [enabled, texture, paneImage]);

    useEffect(() => subscribeTheme(() => setThemeChanges((count) => count + 1)), []);

    useEffect(() => {
        if (!enabled) return;
        const timer = window.setTimeout(() => {
            const palette = readPalette({ shaderDot: grainDotColor(currentTheme()) });
            remoteApi.publishPalette(palette).catch(swallow("publish the theme to paired devices"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [enabled, themeChanges]);

    useEffect(() => {
        const timer = window.setTimeout(() => {
            remoteApi.publishBuddies(JSON.parse(buddies) as PublishedBuddies).catch(swallow("publish buddies to the core"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [buddies]);

    useEffect(() => {
        remoteApi.publishOnScreen(active ? [active] : [], focused).catch(swallow("tell the core which agent is on screen"));
    }, [active, focused]);

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
