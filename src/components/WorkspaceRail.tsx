import { navigateTabs } from "../lib/tabNavigation";
import type { ReactNode } from "react";
import type { RailTab } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { collectPanes } from "../state/layout";
import { AgentRailBody } from "./AgentRail";
import { FileTree } from "./FileTree";
import { RailChanges } from "./rail/RailChanges";
import { SearchPane } from "./SearchPane";
import { IconAgent, IconFolder, IconGit, IconSearch } from "./Icons";

const TABS: { id: RailTab; label: string; icon: ReactNode }[] = [
    { id: "agents", label: "Agents", icon: <IconAgent size={12} /> },
    { id: "files", label: "Files", icon: <IconFolder size={12} /> },
    { id: "changes", label: "Changes", icon: <IconGit size={12} /> },
    { id: "search", label: "Search", icon: <IconSearch size={12} /> },
];

function RailFiles({ cwd }: { cwd: string }) {
    const activePath = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        const windowId = session?.activeWindowId;
        const paneId = windowId ? s.windows[windowId]?.activePaneId : undefined;
        return (paneId ? s.editorViews[paneId]?.activePath : null) ?? null;
    });
    if (!cwd) return <div className="rail-note">open a project to browse files</div>;
    return <FileTree cwd={cwd} activePath={activePath} onOpenFile={(entry) => cmd.requestOpenFile(entry.path)} active />;
}

export function WorkspaceRail() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const density = useStore((s) => s.railDensity);
    const tab = useStore((s) => s.railTab);
    const gitVisible = useStore((s) => {
        const current = s.sessions[s.activeSessionId];
        const window = current && s.windows[current.activeWindowId];
        return (
            current?.view === "windows" &&
            !!window &&
            collectPanes(window.root).some(
                (pane) => pane.kind === "git" && pane.cwd === current.cwd && (!s.zoomedPaneId || s.zoomedPaneId === pane.id),
            )
        );
    });
    const collapsed = tab === "changes" && gitVisible;
    const cwd = session?.cwd ?? "";
    if (!session) return null;

    return (
        <aside className={`workspace-rail${collapsed ? " workspace-rail-git-collapsed" : ""}`} data-density={density}>
            <div className="rail-tabs" role="tablist" aria-label="Workspace">
                {TABS.map((entry) => (
                    <button
                        key={entry.id}
                        type="button"
                        role="tab"
                        aria-selected={tab === entry.id}
                        id={`workspace-tab-${entry.id}`}
                        aria-controls={collapsed && entry.id === "changes" ? undefined : `workspace-panel-${entry.id}`}
                        tabIndex={tab === entry.id ? 0 : -1}
                        onKeyDown={navigateTabs}
                        className={`rail-tab${tab === entry.id ? " active" : ""}`}
                        onClick={(event) => {
                            event.currentTarget.focus();
                            cmd.setRailTab(entry.id);
                        }}>
                        <span className="rail-tab-glyph">{entry.icon}</span>
                        <span>{entry.label}</span>
                    </button>
                ))}
            </div>
            {!collapsed && (
                <div className="rail-body" role="tabpanel" id={`workspace-panel-${tab}`} aria-labelledby={`workspace-tab-${tab}`} tabIndex={0}>
                    {tab === "agents" && <AgentRailBody />}
                    {tab === "files" && <RailFiles cwd={cwd} />}
                    {tab === "changes" && <RailChanges cwd={cwd} />}
                    {tab === "search" && <SearchPane sessionId={session.id} cwd={cwd} active compact visible />}
                </div>
            )}
        </aside>
    );
}
