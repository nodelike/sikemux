import { useEffect, useMemo, useState } from "react";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agents/agentProfiles";
import type { PromptContext } from "../api/acp";
import { agentNamesApi } from "../api/agentNames";
import { wearBuddy } from "../buddies/commands";
import type { RecentChat } from "../api/agents";
import { agentSessionsApi } from "../api/agentSessions";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { agentForChat, agentWindowId, chatKey, ownerSessionId } from "../state/selectors";
import { toAgentBuddy } from "../buddies/agentBuddy";
import { getState, useStore } from "../state/store";
import type { AgentBuddy } from "../state/types";
import { mateNote, type Mate } from "./agentMentions";

const EARLIER_LIMIT = 4;
const EARLIER_SETTLE_MS = 150;
/** The composer waits on the search as the person types, so it answers with what it found by then. */
const EARLIER_BUDGET_MS = 600;

/** An earlier chat of the project, offered after `@` by its title or what was said in it; sending the message opens it again. */
export interface EarlierChat {
    chat: RecentChat;
    /** The project it opens in. */
    sessionId: string;
    /** The closed agent that ran it, when the person named that agent: it opens again under that name and character. */
    named?: { agentId: string; name: string; buddy?: AgentBuddy };
}

/** The project's closed agents the person named, offered after `@` by name while the menu is open. */
export function useClosedNamed(agentId: string, needle: string | null): EarlierChat[] {
    const sessionId = useStore((s) => {
        const windowId = agentWindowId(s, agentId);
        const id = windowId ? ownerSessionId(s, windowId) : null;
        return id && s.sessions[id]?.kind === "project" ? id : null;
    });
    const cwd = useStore((s) => (sessionId ? s.sessions[sessionId]?.cwd || null : null));
    const open = needle !== null;
    const [closed, setClosed] = useState<{ cwd: string; chats: EarlierChat[] } | null>(null);

    useEffect(() => {
        if (!open || !cwd || !sessionId) return;
        let current = true;
        agentNamesApi
            .closed(cwd)
            .then((agents) => {
                if (!current) return;
                const chats = agents.map((agent): EarlierChat => ({
                    chat: { agent: agent.provider, id: agent.sessionId, title: agent.title ?? agent.name, mtime: 0, project: cwd },
                    sessionId,
                    named: { agentId: agent.agentId, name: agent.name, buddy: toAgentBuddy(agent.buddy) ?? undefined },
                }));
                setClosed({ cwd, chats });
            })
            .catch(() => {
                if (current) setClosed({ cwd, chats: [] });
            });
        return () => {
            current = false;
        };
    }, [open, cwd, sessionId]);

    const wanted = needle?.trim().toLowerCase() ?? "";
    return useMemo(
        () => (open && closed?.cwd === cwd ? closed.chats.filter((chat) => chat.named?.name.toLowerCase().startsWith(wanted)) : []),
        [open, closed, cwd, wanted],
    );
}

/** The project's saved chats best matching `needle` by title and by what was said in them, leaving out those open as agents. */
export function useEarlierChats(agentId: string, needle: string | null): EarlierChat[] {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, selections), [profiles, selections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const available = useMemo(
        () =>
            (catalog.data ?? [])
                .filter((agent) => agent.available !== false)
                .map((agent) => agent.type)
                .sort()
                .join(" "),
        [catalog.data],
    );
    const sessionId = useStore((s) => {
        const windowId = agentWindowId(s, agentId);
        const id = windowId ? ownerSessionId(s, windowId) : null;
        return id && s.sessions[id]?.kind === "project" ? id : null;
    });
    const cwd = useStore((s) => (sessionId ? s.sessions[sessionId]?.cwd || null : null));
    const wanted = needle?.trim().toLowerCase() ?? "";
    const [found, setFound] = useState<{ key: string; chats: EarlierChat[] }>({ key: "", chats: [] });
    const key = JSON.stringify([available, cwd, wanted]);

    useEffect(() => {
        if (!wanted || !cwd || !sessionId || !available) return;
        let current = true;
        const timer = window.setTimeout(() => {
            agentSessionsApi
                .search({ query: wanted, projects: [cwd], limit: EARLIER_LIMIT, budgetMs: EARLIER_BUDGET_MS })
                .then((matches) => {
                    if (!current) return;
                    const offered = available.split(" ");
                    const open = new Set(Object.values(getState().agents).map((agent) => chatKey(agent.type, agent.resumeId ?? agent.id)));
                    const chats = matches
                        .filter((found) => offered.includes(found.cli) && !open.has(chatKey(found.cli, found.sessionId)))
                        .map((found): EarlierChat => ({
                            chat: {
                                agent: found.cli,
                                id: found.sessionId,
                                title: found.title,
                                mtime: Math.floor(found.activeAt / 1000),
                                project: found.project,
                            },
                            sessionId,
                        }));
                    setFound({ key, chats });
                })
                .catch(() => {
                    if (current) setFound({ key, chats: [] });
                });
        }, EARLIER_SETTLE_MS);
        return () => {
            current = false;
            window.clearTimeout(timer);
        };
    }, [key, wanted, cwd, sessionId, available]);

    return wanted && found.key === key ? found.chats : [];
}

/** Where a picked earlier chat points until the message is sent. */
export const EARLIER_URI = "sikemux://earlier/";
export const earlierUri = ({ chat }: EarlierChat) => `${EARLIER_URI}${chat.agent}/${chat.id}`;

/** Where an earlier chat opened again for a message points once it has its new name. */
export const WOKEN_URI = "sikemux://woken/";

/**
 * Opens an earlier chat again in a tab behind the one in front, as opening it from Recent does, and tells the agent
 * the message goes to who it is now. It gets a new name for this run, unless the person had named the agent.
 */
export async function wakeEarlier(earlier: EarlierChat): Promise<PromptContext> {
    const { chat, sessionId } = earlier;
    const state = getState();
    const profileId = selectedProviderProfile(chat.agent, state.providerProfiles, state.selectedProviderProfileIds)?.id;
    if (!cmd.addAgent(chat.agent, chat.id, chat.title, { sessionId, profileId, background: true })) {
        throw new Error(`"${chat.title}" could not be opened again`);
    }
    const agent = agentForChat(Object.values(getState().agents), chat);
    if (!agent) throw new Error(`"${chat.title}" could not be opened again`);
    if (agent.launchState === "dormant") cmd.resumeAgent(agent.id);
    let name = agent.name;
    if (!name) {
        const named = await agentNamesApi.name({ agentId: agent.id, cwd: agent.cwd ?? chat.project, provider: agent.type, resumeId: chat.id });
        cmd.setAgentName(agent.id, named.name, named.parent);
        if (named.buddy) wearBuddy(agent.id, named.buddy);
        name = named.name;
    }
    const kept = name.toLowerCase() === earlier.named?.name.toLowerCase();
    const mate: Mate = { id: agent.id, name, project: null, type: agent.type, title: chat.title, buddy: agent.buddy, display: null };
    return { uri: `${WOKEN_URI}${agent.id}`, title: `@${name} · ${chat.title}`, text: mateNote(mate, "Starting", chat.title, kept) };
}
