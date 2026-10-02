import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { receiveForAgent } from "../agents/agentInbox";
import { contextAsText } from "./promptContext";
import { dispatchPaths, resolvePathDropTarget } from "../state/dropRegistry";
import { insertText, textInsertTargetWithin } from "../state/textInsertRegistry";
import type { Agent, ProviderProfile, Session } from "../state/types";
import { acpApi } from "../api/acp";
import { agentSupportsChat } from "../agents/agentLaunch";
import { TerminalPane } from "../terminal/TerminalPane";
import { isResumableSession } from "../terminal/sessionResume";
import { AgentIcon, IconAgent, IconCommand, IconMoreVertical, IconPanelRight, IconPlug } from "../ui/Icons";
import { useStore } from "../state/store";
import { shownDeskPaneId } from "../state/selectors";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import * as cmd from "../state/commands";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { AgentChatPane } from "./AgentChatPane";
import { YoloToggle } from "./YoloToggle";
import { agentCwd, agentPtyContext } from "../agents/agentPtyContext";
import { clearTuiRecovery, relaunchTuiAgent, useTuiResume } from "../agents/tuiResume";
import "../styles/chat.css";

type AgentView = "gui" | "tui";

const WorktreeHeader = lazy(() => import("./WorktreeHeader"));

function DeskButton({ agent }: { agent: Agent }) {
    const open = useStore((state) => shownDeskPaneId(state, agent.id) !== null);
    const label = open ? "Hide desk" : "Show desk";
    const shortcut = useShortcutLabel("desk.toggle");
    return (
        <button
            type="button"
            className="agent-desk-open"
            aria-pressed={open}
            aria-label={label}
            title={withShortcut(label, shortcut)}
            onClick={() => cmd.toggleDesk(agent.id)}>
            <IconPanelRight size={13} />
        </button>
    );
}

function AgentMenuButton({ agent, session, onRename }: { agent: Agent; session: Session; onRename: () => void }) {
    const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null);
    return (
        <>
            <button
                type="button"
                className="agent-surface-menu"
                aria-label="Agent menu"
                aria-haspopup="menu"
                aria-expanded={anchor !== null}
                title="More"
                onClick={(event) => {
                    const box = event.currentTarget.getBoundingClientRect();
                    setAnchor({ x: box.left, y: box.bottom + 4 });
                }}>
                <IconMoreVertical size={14} />
            </button>
            {anchor && (
                <AgentContextMenu agent={agent} session={session} x={anchor.x} y={anchor.y} onClose={() => setAnchor(null)} onRename={onRename} />
            )}
        </>
    );
}

/* Matches the chat's failed-resume row, so both kinds of agent read the same. */
function TuiRecoveryNotice({ agent, profile, cwd, detail }: { agent: Agent; profile?: ProviderProfile; cwd?: string; detail: string | null }) {
    const startNewChat = () =>
        cmd.addAgent(agent.type, undefined, undefined, {
            permissionMode: agent.permissionMode,
            profileId: agent.profileId,
            detectedExecutablePath: profile?.executablePath || agent.executablePath,
            cwd,
        });
    return (
        <div className="agent-tui-recovery">
            <div className="chat-reconnect" role="status">
                <IconPlug size={13} />
                <span>Couldn&apos;t resume this agent</span>
                {detail && <span className="chat-recovery-detail">{detail}</span>}
                <div className="chat-connection-actions">
                    <button type="button" onClick={() => void relaunchTuiAgent(agent.id, agent.ptyId)}>
                        Retry
                    </button>
                    {agent.resumeId && (
                        <button type="button" onClick={startNewChat}>
                            Start new chat
                        </button>
                    )}
                </div>
            </div>
        </div>
    );
}

export function AgentSurface({ agent, session, profile, visible }: { agent: Agent; session: Session; profile?: ProviderProfile; visible: boolean }) {
    const supportsGui = agentSupportsChat(agent.type);
    /* A terminal agent that kept running while the app was closed comes back in its terminal. */
    const [view, setView] = useState<AgentView>(supportsGui && !isResumableSession(agent.ptyId) ? "gui" : "tui");
    const [switching, setSwitching] = useState(false);
    const [chatBusy, setChatBusy] = useState(false);
    const [renaming, setRenaming] = useState(false);
    const { recovery, generation } = useTuiResume(agent.id);
    const cwd = agentCwd(agent, session);

    const switchView = useCallback(
        async (next: AgentView) => {
            if (next === view || switching || (view === "gui" && chatBusy)) return;
            setSwitching(true);
            if (view === "gui") await acpApi.stop(agent.id).catch(() => {});
            clearTuiRecovery(agent.id);
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

    /* A terminal agent takes deliveries as typed text, the way a paste would arrive. */
    const tuiLayer = useRef<HTMLDivElement>(null);
    const tuiShown = visible && view === "tui" && !switching;
    useEffect(() => {
        if (!tuiShown) return;
        return receiveForAgent(agent.id, ({ text, paths, context }) => {
            const target = tuiLayer.current && textInsertTargetWithin(tuiLayer.current);
            if (!target) return;
            const drop = paths?.length ? resolvePathDropTarget(target) : null;
            if (drop && paths) dispatchPaths(drop, paths);
            const typed = [text, ...(context ?? []).map(contextAsText)].filter(Boolean).join("\n\n");
            if (typed) insertText(target, typed);
        });
    }, [agent.id, tuiShown]);

    return (
        <section className="agent-surface">
            <header className="agent-surface-header">
                <span className={`agent-surface-mark agent-glyph ${agent.type}`} aria-hidden="true">
                    <AgentIcon type={agent.type} size={16} />
                </span>
                {renaming ? (
                    <AgentTitleInput
                        title={agent.title}
                        className="agent-surface-title"
                        onSave={(title) => cmd.renameAgent(agent.id, title)}
                        onDone={() => setRenaming(false)}
                    />
                ) : (
                    <span className="agent-surface-title" title={agent.title} onDoubleClick={() => setRenaming(true)}>
                        {agent.title}
                    </span>
                )}
                <AgentMenuButton agent={agent} session={session} onRename={() => setRenaming(true)} />
                {view === "tui" && recovery?.phase === "resuming" && (
                    <span className="agent-surface-resuming" role="status">
                        <span className="chat-activity-loader" aria-hidden="true" />
                        Resuming…
                    </span>
                )}
                {agent.worktree && (
                    <Suspense fallback={null}>
                        <WorktreeHeader agentId={agent.id} worktree={agent.worktree} visible={visible} />
                    </Suspense>
                )}
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
                <DeskButton agent={agent} />
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
                    <div className="agent-tui-layer" ref={tuiLayer}>
                        <TerminalPane
                            key={`${agent.id}:${agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write")}:${generation}`}
                            cwd={cwd}
                            startup={agent.startup}
                            directCommand={agent.directCommand}
                            active={visible}
                            visible={visible}
                            spawnWhen={visible}
                            resumePtyId={agent.ptyId}
                            onPtySession={(id) => cmd.setAgentPty(agent.id, id)}
                            context={agentPtyContext(agent, session)}
                        />
                        {recovery?.phase === "failed" && <TuiRecoveryNotice agent={agent} profile={profile} cwd={cwd} detail={recovery.detail} />}
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
