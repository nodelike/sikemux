import { useEffect, useMemo, useState } from "react";
import { remoteApi, type PublishedChat } from "../api/remote";
import { backdropPicture, grainDotColor } from "../remote/backdrop";
import { usePaneImage } from "../lib/paneImage";
import { readPalette } from "../remote/palette";
import { remoteChats, remoteWorkspace } from "../remote/workspace";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { swallow } from "../state/toast";
import { useStore } from "../state/store";

/** Long enough that opening or renaming several projects publishes once. */
export const PUBLISH_DELAY_MS = 400;

/** While remote access is on, tells the core which projects and agents paired devices may start, the chats they can open, and the theme and backdrop to draw them in. */
export function RemoteWorkspaceBridge() {
    const [enabled, setEnabled] = useState(false);
    const [themeChanges, setThemeChanges] = useState(0);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const profiles = useStore((s) => s.providerProfiles);
    const permissionMode = useStore((s) => s.defaultAgentPermissionMode);
    const published = useMemo(
        () => JSON.stringify(remoteWorkspace(sessions, sessionOrder, profiles, permissionMode)),
        [sessions, sessionOrder, profiles, permissionMode],
    );
    const agents = useStore((s) => s.agents);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const chats = useMemo(
        () => JSON.stringify(remoteChats({ agents, windows, sessions, sessionOrder, windowsBySession })),
        [agents, windows, sessions, sessionOrder, windowsBySession],
    );

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
        if (!enabled) return;
        const timer = window.setTimeout(() => {
            const { projects, launchers } = JSON.parse(published) as ReturnType<typeof remoteWorkspace>;
            remoteApi.publishWorkspace(projects, launchers).catch(swallow("publish projects to paired devices"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [enabled, published]);

    useEffect(() => {
        if (!enabled) return;
        const timer = window.setTimeout(() => {
            remoteApi.publishChats(JSON.parse(chats) as PublishedChat[]).catch(swallow("publish chats to paired devices"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [enabled, chats]);

    return null;
}
