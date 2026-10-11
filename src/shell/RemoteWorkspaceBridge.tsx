import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useRef, useState } from "react";
import { agentSupportsChat } from "../agents/agentLaunch";
import { selectedAgentRuntimeProfiles } from "../agents/agentProfiles";
import { agentApi } from "../api/agents";
import { remoteApi, type PublishedChat } from "../api/remote";
import { getIpcTransport } from "../api/transport";
import { backdropPicture, grainDotColor } from "../remote/backdrop";
import { usePaneImage } from "../lib/paneImage";
import { readPalette } from "../remote/palette";
import { remoteChats, remoteRecent, remoteTitles, remoteWorkspace } from "../remote/workspace";
import { useResource, useResourceEnabled } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import type { AgentType } from "../state/types";
import { activeAgentId } from "../state/selectors";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { swallow } from "../state/toast";
import { useStore } from "../state/store";

/** Long enough that opening or renaming several projects publishes once. */
export const PUBLISH_DELAY_MS = 400;
/** How many of the newest saved chats paired devices can resume. */
export const RECENT_LIMIT = 30;
/** Picks up saved chats written while no change event reached the app. */
export const RECENT_REFRESH_MS = 120_000;

/**
 * Tells the core which projects and agents can be started and what the app calls its agents, for the notch and paired
 * devices, and which agent is on screen. While remote access is on, also the theme and backdrop devices draw in.
 */
export function RemoteWorkspaceBridge() {
    const [enabled, setEnabled] = useState(false);
    const [themeChanges, setThemeChanges] = useState(0);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const profiles = useStore((s) => s.providerProfiles);
    const permissionMode = useStore((s) => s.defaultAgentPermissionMode);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, selections), [profiles, selections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles).data;
    const published = useMemo(
        () => JSON.stringify(remoteWorkspace(sessions, sessionOrder, profiles, permissionMode, catalog)),
        [sessions, sessionOrder, profiles, permissionMode, catalog],
    );
    const agents = useStore((s) => s.agents);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const chats = useMemo(
        () => JSON.stringify(remoteChats({ agents, windows, sessions, sessionOrder, windowsBySession })),
        [agents, windows, sessions, sessionOrder, windowsBySession],
    );
    const titles = useMemo(() => JSON.stringify(remoteTitles({ agents })), [agents]);
    const active = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));
    const focused = useWindowFocused();
    const onScreen = focused ? active : undefined;

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
            const { projects, launchers } = JSON.parse(published) as ReturnType<typeof remoteWorkspace>;
            remoteApi.publishWorkspace(projects, launchers).catch(swallow("publish projects to the core"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [published]);

    useEffect(() => {
        const timer = window.setTimeout(() => {
            remoteApi
                .publishAgents(JSON.parse(chats) as PublishedChat[], JSON.parse(titles) as Record<string, string>)
                .catch(swallow("publish agents to the core"));
        }, PUBLISH_DELAY_MS);
        return () => window.clearTimeout(timer);
    }, [chats, titles]);

    usePublishRecent(enabled);

    useEffect(() => {
        remoteApi.publishOnScreen(onScreen ? [onScreen] : []).catch(swallow("tell the core which agent is on screen"));
    }, [onScreen]);

    return null;
}

/** The newest saved chats across the chat agents and open projects, none of them open, for paired devices to resume. */
function usePublishRecent(enabled: boolean) {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, selections), [profiles, selections]);
    const catalog = useResourceEnabled(enabled, agentCatalogR, runtimeProfiles);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const agents = useStore((s) => s.agents);
    const request = useMemo(() => {
        const providers = (catalog.data ?? [])
            .filter((agent) => agent.available !== false && agentSupportsChat(agent.type))
            .map((agent) => ({ agent: agent.type, configPath: agent.configPath ?? null }));
        const projects = sessionOrder
            .map((id) => sessions[id])
            .filter((session) => session?.kind === "project" && session.cwd)
            .map((session) => session.cwd);
        const exclude = Object.values(agents)
            .map((agent) => ({ agent: agent.type, id: agent.resumeId ?? agent.id }))
            .sort((a, b) => `${a.agent}\0${a.id}`.localeCompare(`${b.agent}\0${b.id}`));
        return JSON.stringify({ providers, projects, exclude });
    }, [catalog.data, sessions, sessionOrder, agents]);
    const profilesRef = useRef({ profiles, selections });
    profilesRef.current = { profiles, selections };

    useEffect(() => {
        if (!enabled) return;
        const { providers, projects, exclude } = JSON.parse(request) as {
            providers: { agent: AgentType; configPath: string | null }[];
            projects: string[];
            exclude: { agent: AgentType; id: string }[];
        };
        const controller = new AbortController();
        let last: string | undefined;
        const publish = () => {
            const page =
                providers.length && projects.length
                    ? agentApi.recent({ providers, projects, limit: RECENT_LIMIT, exclude })
                    : Promise.resolve({ sessions: [] });
            page.then((found) => {
                if (controller.signal.aborted) return;
                const chats = remoteRecent(found.sessions, profilesRef.current.profiles, profilesRef.current.selections);
                const json = JSON.stringify(chats);
                if (json === last) return;
                last = json;
                return remoteApi.publishRecent(chats);
            }).catch(swallow("publish recent chats to paired devices"));
        };
        let timer = window.setTimeout(publish, PUBLISH_DELAY_MS);
        const soon = () => {
            window.clearTimeout(timer);
            timer = window.setTimeout(publish, PUBLISH_DELAY_MS);
        };
        const refresh = window.setInterval(publish, RECENT_REFRESH_MS);
        getIpcTransport()
            .subscribe<{ agent: AgentType; cwd: string }>(
                "agent_sessions_changed",
                ({ payload }) => {
                    if (providers.some((provider) => provider.agent === payload.agent) && projects.includes(payload.cwd)) soon();
                },
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("recent chats listener")(error);
            });
        return () => {
            controller.abort();
            window.clearTimeout(timer);
            window.clearInterval(refresh);
        };
    }, [enabled, request]);
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
