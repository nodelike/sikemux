import { type MouseEvent, type ReactNode, useState } from "react";
import * as cmd from "../state/commands";
import { getState } from "../state/store";
import type { TabRef } from "../state/types";
import { IconClose } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { AgentRowProject } from "./AgentRowTree";
import { type CtxItem, TreeContextMenu } from "./FileTree";
import { HostLabel } from "./RemoteRows";

type Menu = { x: number; y: number; items: CtxItem[] };

/** A tab of the session's strip, as a rail row: it selects the tab, closes it, and opens the strip's menu for it. */
export function TabRow({
    tab,
    icon,
    glyph,
    label,
    detail,
    host,
    title,
    project,
    active,
    onOpen,
    onMenu,
}: {
    tab: TabRef;
    icon: ReactNode;
    /** What colours the icon, the way the strip colours the same tab's. */
    glyph?: string;
    label: string;
    detail?: string | null;
    /** Another host the tab's terminal runs on. */
    host?: string;
    title?: string;
    /** The project the tab lives in, shown under its label when the list spans projects. */
    project?: { name: string; sessionId: string };
    active: boolean;
    /** What a click does instead of selecting the tab in its session. */
    onOpen?: () => void;
    onMenu: (event: MouseEvent) => void;
}) {
    return (
        <div className="agent-row-wrap">
            <button
                type="button"
                className={`agent-row rail-tab${project ? " two-line" : ""}${active ? " active" : ""}`}
                title={title}
                onClick={
                    onOpen ??
                    (() => {
                        if (project && project.sessionId !== getState().activeSessionId) cmd.selectSession(project.sessionId);
                        cmd.selectTab(tab);
                    })
                }
                onAuxClick={(event) => {
                    if (event.button === 1) cmd.closeTab(tab);
                }}
                onContextMenu={onMenu}>
                <span className={`agent-glyph${glyph ? ` ${glyph}` : ""}`}>{icon}</span>
                {project ? (
                    <span className="agent-row-text">
                        <span className="agent-title">{label}</span>
                        <AgentRowProject name={project.name} />
                    </span>
                ) : (
                    <span className="agent-title">{label}</span>
                )}
                <HostLabel host={host} />
                {detail && <span className="rail-file-folder">{detail}</span>}
            </button>
            <Tooltip label={`Close ${label}`}>
                <button type="button" className="row-x" aria-label={`Close ${label}`} onClick={() => cmd.closeTab(tab)}>
                    <IconClose size={11} />
                </button>
            </Tooltip>
        </div>
    );
}

export function useTabMenu() {
    const [menu, setMenu] = useState<Menu | null>(null);
    const open = (event: MouseEvent, items: CtxItem[]) => {
        event.preventDefault();
        event.stopPropagation();
        setMenu({ x: event.clientX, y: event.clientY, items });
    };
    const shown = menu && <TreeContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />;
    return { open, shown };
}
