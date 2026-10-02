import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { agentApi, type AgentInfo, type RecentChat, type RecentCursor } from "../api/agents";
import { getIpcTransport } from "../api/transport";
import { usePageVisible } from "../hooks/usePageVisible";
import { swallow } from "../state/toast";
import type { AgentType } from "../state/types";

export const RECENT_CHATS_PAGE = 12;
const MAX_RELOAD = 100;
const CHANGE_SETTLE_MS = 300;

export interface RecentChatsOptions {
    enabled: boolean;
    providers: AgentInfo[];
    projects: string[];
    /** Chats already open as agents; they are shown as open, not as recent. */
    open: { agent: AgentType; id: string }[];
    query: string;
}

export interface RecentChats {
    chats: RecentChat[];
    status: "loading" | "ready" | "error";
    hasMore: boolean;
    loadMore: () => void;
    /** Shows a new title at once, before the provider has written it. */
    retitle: (agent: AgentType, id: string, title: string) => void;
}

interface Loaded {
    listKey: string;
    chats: RecentChat[];
    next: RecentCursor | null;
    status: RecentChats["status"];
}

const chatKey = (chat: { agent: string; id: string }) => `${chat.agent}\0${chat.id}`;

/**
 * Saved chats a page at a time. Changing what is listed starts again from the
 * top; anything that only changes what is in the list reloads the rows already
 * shown, so the list does not jump back to its first page.
 */
export function useRecentChats({ enabled, providers, projects, open, query }: RecentChatsOptions): RecentChats {
    const needle = query.trim();
    const providerList = useMemo(() => providers.map((provider) => ({ agent: provider.type, configPath: provider.configPath ?? null })), [providers]);
    const listKey = JSON.stringify([providerList, [...projects].sort(), needle]);
    const exclude = useMemo(() => [...open].sort((a, b) => chatKey(a).localeCompare(chatKey(b))), [open]);
    const excludeKey = JSON.stringify(exclude);

    const [loaded, setLoaded] = useState<Loaded>({ listKey: "", chats: [], next: null, status: "loading" });
    const generation = useRef(0);
    const loadingMore = useRef(false);
    const latest = useRef({ providerList, projects, needle, exclude, loaded });
    latest.current = { providerList, projects, needle, exclude, loaded };

    const fetchPage = useCallback((cursor: RecentCursor | null, limit: number) => {
        const { providerList, projects, needle, exclude } = latest.current;
        return agentApi.recent({ providers: providerList, projects, limit, cursor, query: needle || undefined, exclude });
    }, []);

    const reload = useCallback(
        (fromTop: boolean) => {
            const gen = ++generation.current;
            loadingMore.current = false;
            const shown = latest.current.loaded.chats.length;
            const limit = fromTop ? RECENT_CHATS_PAGE : Math.min(MAX_RELOAD, Math.max(RECENT_CHATS_PAGE, shown));
            const key = JSON.stringify([latest.current.providerList, [...latest.current.projects].sort(), latest.current.needle]);
            if (fromTop) setLoaded({ listKey: key, chats: [], next: null, status: "loading" });
            fetchPage(null, limit).then(
                (page) => {
                    if (gen === generation.current) setLoaded({ listKey: key, chats: page.sessions, next: page.next, status: "ready" });
                },
                (error: unknown) => {
                    if (gen !== generation.current) return;
                    setLoaded((current) => ({ ...current, listKey: key, status: "error" }));
                    swallow("recent chats")(error);
                },
            );
        },
        [fetchPage],
    );

    useEffect(() => {
        if (enabled) reload(true);
    }, [enabled, listKey, reload]);

    const lastExclude = useRef(excludeKey);
    useEffect(() => {
        if (lastExclude.current === excludeKey) return;
        lastExclude.current = excludeKey;
        if (enabled) reload(false);
    }, [enabled, excludeKey, reload]);

    const pageVisible = usePageVisible();
    const wasVisible = useRef(pageVisible);
    useEffect(() => {
        if (pageVisible && !wasVisible.current && enabled) reload(false);
        wasVisible.current = pageVisible;
    }, [enabled, pageVisible, reload]);

    useEffect(() => {
        if (!enabled) return;
        const controller = new AbortController();
        let timer: number | undefined;
        void getIpcTransport()
            .subscribe<{ agent: AgentType; cwd: string }>(
                "agent_sessions_changed",
                ({ payload }) => {
                    const { providerList, projects } = latest.current;
                    if (!providerList.some((provider) => provider.agent === payload.agent) || !projects.includes(payload.cwd)) return;
                    window.clearTimeout(timer);
                    timer = window.setTimeout(() => reload(false), CHANGE_SETTLE_MS);
                },
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("recent chats listener")(error);
            });
        return () => {
            controller.abort();
            window.clearTimeout(timer);
        };
    }, [enabled, reload]);

    const loadMore = useCallback(() => {
        const { loaded } = latest.current;
        if (loaded.status !== "ready" || !loaded.next || loadingMore.current) return;
        loadingMore.current = true;
        const gen = generation.current;
        fetchPage(loaded.next, RECENT_CHATS_PAGE).then(
            (page) => {
                if (gen !== generation.current) return;
                loadingMore.current = false;
                setLoaded((current) => {
                    const seen = new Set(current.chats.map(chatKey));
                    return { ...current, chats: [...current.chats, ...page.sessions.filter((chat) => !seen.has(chatKey(chat)))], next: page.next };
                });
            },
            (error: unknown) => {
                if (gen === generation.current) loadingMore.current = false;
                swallow("recent chats")(error);
            },
        );
    }, [fetchPage]);

    const retitle = useCallback((agent: AgentType, id: string, title: string) => {
        setLoaded((current) => ({
            ...current,
            chats: current.chats.map((chat) => (chat.agent === agent && chat.id === id ? { ...chat, title } : chat)),
        }));
    }, []);

    const current = loaded.listKey === listKey;
    return {
        chats: current ? loaded.chats : [],
        status: current ? loaded.status : "loading",
        hasMore: current && loaded.next !== null,
        loadMore,
        retitle,
    };
}
