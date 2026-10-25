import { useMemo } from "react";
import { agentHandle } from "../agents/AgentLabel";
import { agentLabel, isAgentType } from "@shared/agents/catalog";
import type { ChatInfo, SessionInfo } from "@shared/wire.generated";
import { chatsIn, hostTitle, newWorkHost, openRemoteChat, openRemoteTerminal, hostProjectOf, terminalsIn, type HostProject } from "../hosts/hosts";
import { useStore } from "../state/store";
import type { Session } from "../state/types";
import { AgentIcon, IconAgent, IconTerminal } from "../ui/Icons";
import { AgentRowProject } from "./AgentRowTree";

/** The project a session on another host is, as that host shows it. */
export function useHostProject(session: Session | undefined): HostProject | undefined {
    const hosts = useStore((s) => s.hosts);
    const hostViews = useStore((s) => s.hostViews);
    return useMemo(() => (session ? hostProjectOf({ hosts, hostViews }, session) : undefined), [session, hosts, hostViews]);
}

/** Which host the new chat and terminal rows start on, said only when it is not this one. */
export function NewOnHost() {
    const name = useStore((s) => {
        const here = newWorkHost(s);
        return here.kind === "core" && !here.host ? null : hostTitle(s, here);
    });
    return name ? <div className="agent-actions-host">on {name}</div> : null;
}

/** A chat running on another host that this app does not show yet, with its project when the list spans projects. */
export function RemoteChatRow({ chat, project, onOpen }: { chat: ChatInfo; project?: string; onOpen: () => void }) {
    return (
        <div className="agent-row-wrap">
            <button type="button" className={`agent-row${project ? " two-line" : ""}`} onClick={onOpen}>
                <span className={`agent-glyph ${chat.provider}`}>
                    {isAgentType(chat.provider) ? <AgentIcon type={chat.provider} size={20} /> : <IconAgent size={20} />}
                </span>
                <span className="agent-row-text">
                    <span className="agent-title">{chat.title ?? agentLabel(chat.provider)}</span>
                    {project && <AgentRowProject name={project} />}
                </span>
                {chat.name && <span className="agent-row-name">{agentHandle(chat.name)}</span>}
            </button>
        </div>
    );
}

/** The chats running in a project on another host that are not open here. */
export function remoteChatRows({ sessionId, place, open }: { sessionId: string; place: HostProject | undefined; open: ReadonlySet<string> }) {
    if (!place) return [];
    return chatsIn(place)
        .filter((chat) => !open.has(chat.agentId))
        .map((chat) => <RemoteChatRow key={chat.agentId} chat={chat} onOpen={() => openRemoteChat(sessionId, place.host.id, chat)} />);
}

/** A terminal running on another host that this app does not show yet. */
export function RemoteTerminalRow({ terminal, project, onOpen }: { terminal: SessionInfo; project?: string; onOpen: () => void }) {
    const title = terminal.windowTitle || terminal.title || "Terminal";
    return (
        <div className="agent-row-wrap">
            <button type="button" className={`agent-row rail-tab${project ? " two-line" : ""}`} onClick={onOpen}>
                <span className="agent-glyph term">
                    <IconTerminal size={15} />
                </span>
                {project ? (
                    <span className="agent-row-text">
                        <span className="agent-title">{title}</span>
                        <AgentRowProject name={project} />
                    </span>
                ) : (
                    <span className="agent-title">{title}</span>
                )}
            </button>
        </div>
    );
}

/** The terminals running in a project on another host that are not open here. */
export function remoteTerminalRows({ sessionId, place, open }: { sessionId: string; place: HostProject | undefined; open: ReadonlySet<number> }) {
    if (!place) return [];
    return terminalsIn(place)
        .filter((terminal) => !open.has(terminal.id))
        .map((terminal) => (
            <RemoteTerminalRow
                key={terminal.id}
                terminal={terminal}
                onOpen={() => openRemoteTerminal(sessionId, place.host.id, terminal, place.project.path)}
            />
        ));
}
