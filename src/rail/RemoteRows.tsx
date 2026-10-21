import { useMemo } from "react";
import { agentHandle } from "../agents/AgentLabel";
import { agentLabel, isAgentType } from "../agents/catalog";
import type { ChatInfo, SessionInfo } from "../api/generated/wire";
import {
    chatsIn,
    hostName,
    hostTitle,
    newWorkHost,
    openRemoteChat,
    openRemoteTerminal,
    remoteCopiesOf,
    terminalsIn,
    type RemoteCopy,
} from "../hosts/hosts";
import { useStore } from "../state/store";
import type { Session } from "../state/types";
import { AgentIcon, IconAgent, IconTerminal } from "../ui/Icons";
import { AgentRowProject } from "./AgentRowTree";

/** The quiet note on a chat or terminal that runs on another host. */
export function HostLabel({ host }: { host: string | undefined }) {
    const name = useStore((s) => hostName(s, host));
    return name ? <span className="agent-row-host">on {name}</span> : null;
}

/** The copies of the session's project on the other hosts the app reaches. */
export function useRemoteCopies(session: Session | undefined): RemoteCopy[] {
    const hosts = useStore((s) => s.hosts);
    const hostViews = useStore((s) => s.hostViews);
    const projectIdentities = useStore((s) => s.projectIdentities);
    return useMemo(
        () => (session ? remoteCopiesOf({ hosts, hostViews, projectIdentities }, session) : []),
        [session, hosts, hostViews, projectIdentities],
    );
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
export function RemoteChatRow({ chat, project, host, onOpen }: { chat: ChatInfo; project?: string; host?: string; onOpen: () => void }) {
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
                {host && <span className="agent-row-host">on {host}</span>}
                {chat.name && <span className="agent-row-name">{agentHandle(chat.name)}</span>}
            </button>
        </div>
    );
}

/** The chats running in the project's copies elsewhere that are not open here. */
export function remoteChatRows({ sessionId, copies, open }: { sessionId: string; copies: readonly RemoteCopy[]; open: ReadonlySet<string> }) {
    return copies.flatMap((copy) =>
        chatsIn(copy)
            .filter((chat) => !open.has(chat.agentId))
            .map((chat) => (
                <RemoteChatRow key={chat.agentId} chat={chat} host={copy.host.name} onOpen={() => openRemoteChat(sessionId, copy.host.id, chat)} />
            )),
    );
}

/** A terminal running on another host that this app does not show yet. */
export function RemoteTerminalRow({
    terminal,
    project,
    host,
    onOpen,
}: {
    terminal: SessionInfo;
    project?: string;
    host?: string;
    onOpen: () => void;
}) {
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
                {host && <span className="agent-row-host">on {host}</span>}
            </button>
        </div>
    );
}

/** The terminals running in the project's copies elsewhere that are not open here. */
export function remoteTerminalRows({ sessionId, copies, open }: { sessionId: string; copies: readonly RemoteCopy[]; open: ReadonlySet<number> }) {
    return copies.flatMap((copy) =>
        terminalsIn(copy)
            .filter((terminal) => !open.has(terminal.id))
            .map((terminal) => (
                <RemoteTerminalRow
                    key={terminal.id}
                    terminal={terminal}
                    host={copy.host.name}
                    onOpen={() => openRemoteTerminal(sessionId, copy.host.id, terminal, copy.project.path)}
                />
            )),
    );
}
