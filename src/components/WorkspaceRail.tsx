import type { ReactNode } from "react";
import type { RailTab } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { AgentRailBody } from "./AgentRail";
import { FileTree } from "./FileTree";
import { RailChanges } from "./rail/RailChanges";
import { IconAgent, IconFolder, IconGit } from "./Icons";
import { useResourceEnabled } from "../state/resources";
import { gitStatusR } from "../state/resources.defs";

const TABS: { id: RailTab; label: string; icon: ReactNode }[] = [
    { id: "agents", label: "Agents", icon: <IconAgent size={12} /> },
    { id: "files", label: "Files", icon: <IconFolder size={12} /> },
    { id: "changes", label: "Changes", icon: <IconGit size={12} /> },
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
    const cwd = session?.cwd ?? "";
    const isProject = session?.kind === "project";
    const status = useResourceEnabled(isProject && !!cwd && tab !== "changes", gitStatusR, cwd || "");
    const changeCount = status.data?.files.length ?? 0;

    if (!session) return null;

    return (
        <aside className="workspace-rail" data-density={density}>
            <div className="rail-tabs" role="tablist" aria-label="Workspace">
                {TABS.map((entry) => (
                    <button
                        key={entry.id}
                        type="button"
                        role="tab"
                        aria-selected={tab === entry.id}
                        className={`rail-tab${tab === entry.id ? " active" : ""}`}
                        onClick={() => cmd.setRailTab(entry.id)}>
                        <span className="rail-tab-glyph">{entry.icon}</span>
                        <span>{entry.label}</span>
                        {entry.id === "changes" && changeCount > 0 && <span className="rail-tab-count">{changeCount}</span>}
                    </button>
                ))}
            </div>
            <div className="rail-body">
                {tab === "agents" && <AgentRailBody />}
                {tab === "files" && <RailFiles cwd={cwd} />}
                {tab === "changes" && <RailChanges cwd={cwd} />}
            </div>
        </aside>
    );
}
