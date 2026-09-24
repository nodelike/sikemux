import { useCallback, useState } from "react";
import type { Agent, ProviderProfile, Session } from "../state/types";
import { acpApi } from "../api/acp";
import { TerminalPane } from "../terminal/TerminalPane";
import { IconAgent, IconCommand, IconGlobe } from "../components/Icons";
import { useStore } from "../state/store";
import { shownBrowserPaneId } from "../state/selectors";
import * as cmd from "../state/commands";
import { AgentChatPane } from "./AgentChatPane";
import { YoloToggle } from "./YoloToggle";
import "../styles/chat.css";

type AgentView = "gui" | "tui";

function BrowserButton({ agent }: { agent: Agent }) {
    const open = useStore((state) => shownBrowserPaneId(state, agent.id) !== null);
    const label = open ? "Hide browser" : "Show browser";
    return (
        <button
            type="button"
            className="agent-browser-open"
            aria-pressed={open}
            aria-label={label}
            title={label}
            onClick={() => cmd.toggleBrowserPane(agent.id)}>
            <IconGlobe size={13} />
        </button>
    );
}

export function AgentSurface({ agent, session, profile, visible }: { agent: Agent; session: Session; profile?: ProviderProfile; visible: boolean }) {
    const supportsGui = agent.type === "claude" || agent.type === "codex";
    const [view, setView] = useState<AgentView>(supportsGui ? "gui" : "tui");
    const [switching, setSwitching] = useState(false);
    const [chatBusy, setChatBusy] = useState(false);

    const switchView = useCallback(
        async (next: AgentView) => {
            if (next === view || switching || (view === "gui" && chatBusy)) return;
            setSwitching(true);
            if (view === "gui") await acpApi.stop(agent.id).catch(() => {});
            setView(next);
            window.requestAnimationFrame(() => setSwitching(false));
        },
        [agent.id, chatBusy, switching, view],
    );

    /* The window layer stays mounted so a live agent keeps its process, so the
       session connects as soon as the pane exists rather than when it is first
       looked at: the adapter and CLI take about a second to come up, and that
       second should be spent before the user switches to this agent. */
    const guiActive = supportsGui && view === "gui" && !switching;

    return (
        <section className="agent-surface">
            <header className="agent-surface-header">
                <span className="agent-surface-title" title={agent.title}>
                    {agent.title}
                </span>
                {view === "tui" && cmd.agentSupportsSkipPermissions(agent.type) && <YoloToggle agent={agent} relaunches />}
                <div className="agent-view-switch" role="group" aria-label="Agent view">
                    <button
                        type="button"
                        aria-pressed={view === "gui"}
                        disabled={!supportsGui || switching}
                        title="Open the built-in agent chat"
                        onClick={() => void switchView("gui")}>
                        <IconAgent size={13} />
                        <span>GUI</span>
                    </button>
                    <button
                        type="button"
                        aria-pressed={view === "tui"}
                        disabled={switching || (view === "gui" && chatBusy)}
                        title={chatBusy ? "Stop the current turn before opening TUI" : "Open native agent TUI"}
                        onClick={() => void switchView("tui")}>
                        <IconCommand size={13} />
                        <span>TUI</span>
                    </button>
                </div>
                <BrowserButton agent={agent} />
            </header>

            <div className="agent-surface-body">
                {supportsGui && (
                    <div className={`agent-gui-layer${view === "gui" ? " visible" : ""}`}>
                        <AgentChatPane
                            agent={agent}
                            profile={profile}
                            cwd={agent.cwd || session.cwd}
                            active={guiActive}
                            visible={visible && guiActive}
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
