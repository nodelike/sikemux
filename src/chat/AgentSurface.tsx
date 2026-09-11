import { useCallback, useEffect, useState } from "react";
import type { Agent, ProviderProfile, Session } from "../state/types";
import { acpApi } from "../api/acp";
import { TerminalPane } from "../terminal/TerminalPane";
import { IconAgent, IconCommand } from "../components/Icons";
import { AgentChatPane } from "./AgentChatPane";
import "../styles/chat.css";

type AgentView = "session" | "tui";

export function AgentSurface({ agent, session, profile, visible }: { agent: Agent; session: Session; profile?: ProviderProfile; visible: boolean }) {
    const supportsSession = agent.type === "claude" || agent.type === "codex";
    const [opened, setOpened] = useState(visible);
    useEffect(() => {
        if (visible) setOpened(true);
    }, [visible]);
    const [view, setView] = useState<AgentView>(supportsSession ? "session" : "tui");
    const [switching, setSwitching] = useState(false);
    const [chatBusy, setChatBusy] = useState(false);

    const switchView = useCallback(
        async (next: AgentView) => {
            if (next === view || switching || (view === "session" && chatBusy)) return;
            setSwitching(true);
            if (view === "session") await acpApi.stop(agent.id).catch(() => {});
            setView(next);
            window.requestAnimationFrame(() => setSwitching(false));
        },
        [agent.id, chatBusy, switching, view],
    );

    const sessionActive = supportsSession && view === "session" && !switching;

    return (
        <section className="agent-surface">
            <header className="agent-surface-header">
                <span className="agent-surface-title" title={agent.title}>
                    {agent.title}
                </span>
                <div className="agent-view-switch" role="group" aria-label="Agent view">
                    <button
                        type="button"
                        aria-pressed={view === "session"}
                        disabled={!supportsSession || switching}
                        onClick={() => void switchView("session")}>
                        <IconAgent size={13} />
                        <span>Session</span>
                    </button>
                    <button
                        type="button"
                        aria-pressed={view === "tui"}
                        disabled={switching || (view === "session" && chatBusy)}
                        title={chatBusy ? "Stop the current turn before opening TUI" : "Open native agent TUI"}
                        onClick={() => void switchView("tui")}>
                        <IconCommand size={13} />
                        <span>TUI</span>
                    </button>
                </div>
            </header>

            <div className="agent-surface-body">
                {supportsSession && (
                    <div className={`agent-session-layer${view === "session" ? " visible" : ""}`}>
                        <AgentChatPane
                            agent={agent}
                            profile={profile}
                            cwd={agent.cwd || session.cwd}
                            active={opened && sessionActive}
                            visible={visible && sessionActive}
                            onBusyChange={setChatBusy}
                        />
                    </div>
                )}
                {view === "tui" && !switching && (
                    <div className="agent-tui-layer">
                        <TerminalPane
                            key={`${agent.id}:${agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write")}`}
                            cwd={agent.cwd || session.cwd || undefined}
                            startup={agent.startup}
                            directCommand={agent.directCommand}
                            active={visible}
                            visible={visible}
                            spawnWhen={visible}
                            context={{
                                sessionId: session.id,
                                sessionName: session.name,
                                sessionKind: session.kind,
                                ...(session.kind === "project" && (agent.cwd || session.cwd) ? { project: agent.cwd || session.cwd } : {}),
                                agentId: agent.id,
                                agentType: agent.type,
                            }}
                        />
                    </div>
                )}
                {switching && (
                    <div className="agent-transport-switching" role="status">
                        Switching agent view…
                    </div>
                )}
            </div>
        </section>
    );
}
