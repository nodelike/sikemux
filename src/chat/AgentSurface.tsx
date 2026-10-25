import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { receiveForAgent } from "../agents/agentInbox";
import { contextAsText } from "@shared/chat/promptContext";
import { dispatchPaths, resolvePathDropTarget } from "../state/dropRegistry";
import { insertText, textInsertTargetWithin } from "../state/textInsertRegistry";
import type { Agent, ProviderProfile, Session } from "../state/types";
import { acpApi } from "../api/acp";
import { effectiveMode } from "@shared/agents/agentLaunch";
import { canBypassPermissions, agentSupportsChat } from "@shared/agents/catalog";
import { TerminalPane } from "../terminal/TerminalPane";
import { putDraft, takeDraft } from "./draftHandoff";
import { ptyCall } from "../api/core";
import { isResumableSession, offerResumableSessions } from "../terminal/sessionResume";
import { AgentBuddy } from "../buddies/Buddy";
import { openBuddyPicker } from "../buddies/commands";
import { AgentIcon, IconAgent, IconMoreVertical, IconPanelRight, IconTerminal } from "@shared/ui/Icons";
import { useStore } from "../state/store";
import { shownDeskPaneId } from "../state/selectors";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentLabel } from "../agents/AgentLabel";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import * as cmd from "../state/commands";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { AgentChatPane } from "./AgentChatPane";
import { YoloToggle } from "./YoloToggle";
import { ShareButton } from "../share/ShareControls";
import { agentCwd, agentPtyContext } from "../agents/agentPtyContext";
import "../styles/chat.css";
import { swallow } from "../state/toast";

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

function AgentMenuButton({ agent, session, onRetitle }: { agent: Agent; session: Session; onRetitle: () => void }) {
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
                <AgentContextMenu agent={agent} session={session} x={anchor.x} y={anchor.y} onClose={() => setAnchor(null)} onRetitle={onRetitle} />
            )}
        </>
    );
}

/* Matches the chat's failed-resume row, so both kinds of agent read the same. */
export function AgentSurface({ agent, session, profile, visible }: { agent: Agent; session: Session; profile?: ProviderProfile; visible: boolean }) {
    const supportsGui = agentSupportsChat(agent.type);
    /* A terminal agent that kept running while the app was closed comes back in its terminal. */
    const [view, setView] = useState<AgentView>(supportsGui && !isResumableSession(agent.ptyId) ? "gui" : "tui");
    const [switching, setSwitching] = useState(false);
    /* What was typed in the chat when the agent moved to its TUI, left at the TUI's prompt. */
    const [carried, setCarried] = useState("");
    const [chatBusy, setChatBusy] = useState(false);
    const working = useStore((state) => state.agentStatus[agent.id]?.group === "working");
    const busy = chatBusy || working;
    const [renaming, setRenaming] = useState(false);
    const buddies = useStore((state) => state.faceStyle.on);
    /* The core starts a terminal agent again after a crash, in a terminal of its own that the pane takes up. */
    const [terminal, setTerminal] = useState({ ptyId: agent.ptyId, generation: 0 });
    useEffect(() => {
        if (agent.ptyId === undefined || agent.ptyId === terminal.ptyId) return;
        offerResumableSessions([agent.ptyId]);
        setTerminal(({ generation }) => ({ ptyId: agent.ptyId, generation: generation + 1 }));
    }, [agent.ptyId, terminal.ptyId]);
    const cwd = agentCwd(agent, session);

    const tuiLayer = useRef<HTMLDivElement>(null);
    const switchView = useCallback(
        async (next: AgentView) => {
            if (next === view || switching || busy) return;
            setSwitching(true);
            setCarried(next === "tui" ? takeDraft(agent.id) : "");
            if (next === "gui" && terminal.ptyId !== undefined) {
                const read = await ptyCall<{ draft: string }>(terminal.ptyId, "promptDraft").catch(swallow("read the TUI's draft"));
                putDraft(agent.id, read?.draft ?? "");
            }
            if (view === "gui") await acpApi.stop(agent.id).catch(swallow("stop the chat agent"));
            setView(next);
            window.requestAnimationFrame(() => setSwitching(false));
        },
        [agent.id, busy, switching, terminal.ptyId, view],
    );

    /* The window layer stays mounted so a live agent keeps its process, so the
       session connects as soon as the pane exists rather than when it is first
       looked at: the adapter and CLI take about a second to come up, and that
       second should be spent before the user switches to this agent. */
    const guiActive = supportsGui && view === "gui" && !switching;

    /* A terminal agent takes deliveries as typed text, the way a paste would arrive. */
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
        <section className="agent-surface" data-view={view}>
            <header className="agent-surface-header">
                <div className="agent-surface-name">
                    {buddies ? (
                        <button
                            type="button"
                            className="agent-surface-mark agent-glyph buddy-mark"
                            title="Change buddy"
                            aria-label="Change buddy"
                            onClick={() => openBuddyPicker(agent.id)}>
                            <AgentBuddy agent={agent} size={17} />
                        </button>
                    ) : (
                        <span className={`agent-surface-mark agent-glyph ${agent.type}`} aria-hidden="true">
                            <AgentIcon type={agent.type} size={16} />
                        </span>
                    )}
                    {renaming ? (
                        <AgentTitleInput
                            title={agent.title}
                            className="agent-surface-title"
                            onSave={(title) => cmd.renameAgent(agent.id, title)}
                            onDone={() => setRenaming(false)}
                        />
                    ) : (
                        <span className="agent-surface-title" title={agent.title} onDoubleClick={() => setRenaming(true)}>
                            <AgentLabel agent={agent} />
                        </span>
                    )}
                    {supportsGui && <ShareButton agent={agent} />}
                    <AgentMenuButton agent={agent} session={session} onRetitle={() => setRenaming(true)} />
                </div>
                <div className="agent-surface-end">
                    <div className="agent-surface-controls">
                        {view === "tui" && agent.worktree && (
                            <Suspense fallback={null}>
                                <WorktreeHeader agentId={agent.id} worktree={agent.worktree} visible={visible} />
                            </Suspense>
                        )}
                        {view === "tui" && canBypassPermissions(agent.type) && <YoloToggle agent={agent} relaunches />}
                        <div className="agent-view-switch" role="group" aria-label="Agent view">
                            <button
                                type="button"
                                aria-pressed={view === "gui"}
                                disabled={!supportsGui || switching || (view === "tui" && busy)}
                                title={view === "tui" && busy ? "Wait for the agent to finish before opening GUI" : "Open the built-in agent chat"}
                                onClick={() => void switchView("gui")}>
                                <IconAgent size={13} />
                                <span>GUI</span>
                            </button>
                            <button
                                type="button"
                                aria-pressed={view === "tui"}
                                disabled={switching || (view === "gui" && busy)}
                                title={view === "gui" && busy ? "Wait for the agent to finish before opening TUI" : "Open native agent TUI"}
                                onClick={() => void switchView("tui")}>
                                <IconTerminal size={13} />
                                <span>TUI</span>
                            </button>
                        </div>
                    </div>
                    <DeskButton agent={agent} />
                </div>
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
                            key={`${agent.id}:${effectiveMode(agent)}:${terminal.generation}`}
                            cwd={cwd}
                            initialDraft={carried || undefined}
                            onInitialInputDelivered={() => setCarried("")}
                            active={visible}
                            visible={visible}
                            spawnWhen={visible}
                            resumePtyId={agent.ptyId}
                            onPtySession={(id) => setTerminal((shown) => ({ ...shown, ptyId: id }))}
                            context={agentPtyContext(agent, session)}
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
