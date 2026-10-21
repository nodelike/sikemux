import { useState } from "react";
import type { AgentInfo, RecentChat } from "../api/agents";
import { canDeleteChats } from "../agents/catalog";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import * as cmd from "../state/commands";
import { chatKey, projectSessionId } from "../state/selectors";
import { useStore } from "../state/store";
import { AgentIcon, IconMessage, IconPencil, IconPin, IconTrash } from "../ui/Icons";
import { Panel, PanelHeader } from "../ui/Panel";
import { TreeContextMenu } from "./FileTree";
import type { RecentChats } from "./useRecentChats";
import { formatAge } from "../lib/format";
import { basename } from "../lib/paths";
import { AgentRowProject } from "./AgentRowTree";

/** Saved chats, each resumed in the project it ran in. */
export function RecentChatList({ recent, providers }: { recent: RecentChats; providers: AgentInfo[] }) {
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const allProjects = useStore((s) => s.tabRailScope === "all");
    const [renaming, setRenaming] = useState<string | null>(null);
    const [menu, setMenu] = useState<{ chat: RecentChat; x: number; y: number } | null>(null);

    if (recent.chats.length === 0) return null;

    const providerOf = (chat: RecentChat) => providers.find((provider) => provider.type === chat.agent);
    const projectOf = (chat: RecentChat) => {
        const id = projectSessionId({ sessionOrder, sessions }, chat.project);
        return id ? sessions[id] : undefined;
    };

    const open = (chat: RecentChat) => cmd.openSavedChat(chat, { detectedExecutablePath: providerOf(chat)?.command });

    const rename = (chat: RecentChat, title: string) => {
        const provider = providerOf(chat);
        cmd.renameAgentSession(
            {
                type: chat.agent,
                cwd: chat.cwd ?? chat.project,
                sessionId: chat.id,
                configPath: provider?.configPath ?? undefined,
                executablePath: provider?.command,
            },
            title,
        );
        recent.retitle(chat.agent, chat.id, title);
    };

    const remove = (chat: RecentChat) => {
        const provider = providerOf(chat);
        const session = {
            type: chat.agent,
            cwd: chat.cwd ?? chat.project,
            sessionId: chat.id,
            configPath: provider?.configPath ?? undefined,
            executablePath: provider?.command,
        };
        void cmd.deleteAgentSession(session, chat.title).then((deleted) => {
            if (deleted) recent.forget(chat.agent, chat.id);
        });
    };

    return (
        <Panel variant="group" className="agent-group">
            <PanelHeader label="Recent" rule />
            {recent.chats.map((chat) => {
                const key = chatKey(chat.agent, chat.id);
                const glyph = (
                    <span className={`agent-glyph ${chat.agent}`}>
                        <AgentIcon type={chat.agent} size={20} />
                    </span>
                );
                const project = projectOf(chat);
                const elsewhere = project?.id !== activeSessionId ? `${project?.name ?? chat.project} — ${chat.title}` : undefined;
                return renaming === key ? (
                    <div key={key} className="agent-row recent">
                        {glyph}
                        <AgentTitleInput
                            title={chat.title}
                            className="agent-title"
                            onSave={(value) => rename(chat, value)}
                            onDone={() => setRenaming(null)}
                        />
                        <span className="agent-ago">{formatAge(chat.mtime * 1000)}</span>
                    </div>
                ) : (
                    <div key={key} className="agent-recent">
                        <button
                            className={`agent-row recent${allProjects ? " two-line" : ""}`}
                            title={elsewhere}
                            onClick={() => open(chat)}
                            onContextMenu={(event) => {
                                event.preventDefault();
                                setMenu({ chat, x: event.clientX, y: event.clientY });
                            }}>
                            {glyph}
                            <span className="agent-row-text">
                                <span className="agent-title">{chat.title}</span>
                                {allProjects && <AgentRowProject name={project?.name ?? basename(chat.project)} />}
                            </span>
                            <span className="agent-ago">{formatAge(chat.mtime * 1000)}</span>
                        </button>
                    </div>
                );
            })}
            {menu && (
                <TreeContextMenu
                    x={menu.x}
                    y={menu.y}
                    items={[
                        { label: "Open", icon: <IconMessage size={13} />, run: () => open(menu.chat) },
                        { label: "Rename…", icon: <IconPencil size={13} />, run: () => setRenaming(chatKey(menu.chat.agent, menu.chat.id)) },
                        {
                            label: "Pin",
                            icon: <IconPin size={13} />,
                            run: () => cmd.pinChat({ agent: menu.chat.agent, id: menu.chat.id, title: menu.chat.title, project: menu.chat.project }),
                        },
                        ...(canDeleteChats(menu.chat.agent)
                            ? [{ sep: true }, { label: "Delete…", icon: <IconTrash size={13} />, danger: true, run: () => remove(menu.chat) }]
                            : []),
                    ]}
                    onClose={() => setMenu(null)}
                />
            )}
        </Panel>
    );
}
