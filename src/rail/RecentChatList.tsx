import { useState } from "react";
import type { AgentInfo, RecentChat } from "../api/agents";
import { selectedProviderProfile } from "../agents/agentProfiles";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { AgentIcon } from "../ui/Icons";
import { Panel, PanelHeader } from "../ui/Panel";
import { TreeContextMenu } from "./FileTree";
import type { RecentChats } from "./useRecentChats";

function ago(unixSecs: number): string {
    if (!unixSecs) return "";
    const d = Math.max(0, Date.now() / 1000 - unixSecs);
    if (d < 90) return "now";
    if (d < 3600) return `${Math.round(d / 60)}m`;
    if (d < 86400) return `${Math.round(d / 3600)}h`;
    return `${Math.round(d / 86400)}d`;
}

const chatKey = (chat: RecentChat) => `${chat.agent}\0${chat.id}`;

/** Saved chats, each resumed in the project it ran in. */
export function RecentChatList({ recent, providers }: { recent: RecentChats; providers: AgentInfo[] }) {
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const [renaming, setRenaming] = useState<string | null>(null);
    const [menu, setMenu] = useState<{ chat: RecentChat; x: number; y: number } | null>(null);

    if (recent.chats.length === 0) return null;

    const providerOf = (chat: RecentChat) => providers.find((provider) => provider.type === chat.agent);
    const projectOf = (chat: RecentChat) =>
        sessionOrder.map((id) => sessions[id]).find((session) => session?.kind === "project" && session.cwd === chat.project);

    const open = (chat: RecentChat) => {
        const project = projectOf(chat);
        cmd.addAgent(chat.agent, chat.id, chat.title, {
            sessionId: project?.id,
            profileId: selectedProviderProfile(chat.agent, profiles, profileSelections)?.id,
            detectedExecutablePath: providerOf(chat)?.command,
        });
        if (project && project.id !== activeSessionId) cmd.selectSession(project.id);
    };

    const rename = (chat: RecentChat, title: string) => {
        const provider = providerOf(chat);
        cmd.renameAgentSession(
            {
                type: chat.agent,
                cwd: chat.project,
                sessionId: chat.id,
                configPath: provider?.configPath ?? undefined,
                executablePath: provider?.command,
            },
            title,
        );
        recent.retitle(chat.agent, chat.id, title);
    };

    return (
        <Panel variant="group" className="agent-group">
            <PanelHeader label="Recent" rule />
            {recent.chats.map((chat) => {
                const key = chatKey(chat);
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
                        <span className="agent-ago">{ago(chat.mtime)}</span>
                    </div>
                ) : (
                    <button
                        key={key}
                        className="agent-row recent"
                        title={elsewhere}
                        onClick={() => open(chat)}
                        onContextMenu={(event) => {
                            event.preventDefault();
                            setMenu({ chat, x: event.clientX, y: event.clientY });
                        }}>
                        {glyph}
                        <span className="agent-title">{chat.title}</span>
                        <span className="agent-ago">{ago(chat.mtime)}</span>
                    </button>
                );
            })}
            {menu && (
                <TreeContextMenu
                    x={menu.x}
                    y={menu.y}
                    items={[
                        { label: "Open", run: () => open(menu.chat) },
                        { label: "Rename…", run: () => setRenaming(chatKey(menu.chat)) },
                    ]}
                    onClose={() => setMenu(null)}
                />
            )}
        </Panel>
    );
}
