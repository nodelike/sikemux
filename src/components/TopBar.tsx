import { memo, useEffect, useState, type MouseEvent as ReactMouseEvent } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useBattery } from "../hooks/useBattery";
import { useClock } from "../hooks/useClock";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { swallow } from "../state/toast";
import { gitOverviewR } from "../state/resources.defs";
import { useInstalledPlugins } from "../plugins/installed";
import { useStore } from "../state/store";
import { activeAgentId } from "../state/selectors";
import { IconAgent, IconBattery, IconChevron, IconCommand, IconFocus, IconFolder, IconGit, IconPanelLeft, IconZoom, WindowIcon } from "./Icons";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import { Tooltip } from "./Tooltip";
import { isUpdateBusy, updateDownloadPercent, updateStatusLabel } from "../api/updater";

const time2 = (n: number) => String(n).padStart(2, "0");

const TOP_BAR_NO_DRAG_SELECTOR = [
    "button",
    "a[href]",
    "input",
    "select",
    "textarea",
    "[contenteditable='true']",
    "[role='button']",
    "[data-no-window-drag]",
].join(",");

function isTopBarNoDragTarget(target: EventTarget | null, root: HTMLElement): boolean {
    if (!(target instanceof Element)) return false;
    const interactive = target.closest(TOP_BAR_NO_DRAG_SELECTOR);
    return !!interactive && root.contains(interactive);
}

function startWindowDragFromTopBar(e: ReactMouseEvent<HTMLElement>) {
    if (e.button !== 0 || e.defaultPrevented) return;
    if (isTopBarNoDragTarget(e.target, e.currentTarget)) return;
    e.preventDefault();
    void getCurrentWindow().startDragging().catch(swallow("startDragging"));
}

function twelveHour(d: Date): { h: number; m: number; ap: "am" | "pm" } {
    const h24 = d.getHours();
    const h = h24 % 12 || 12;
    return { h, m: d.getMinutes(), ap: h24 >= 12 ? "pm" : "am" };
}

/*
 * The branch and its counts come off the overview the rest of the app already
 * reads. Asking for a status of its own made the backend do the recursive
 * untracked walk a second time on every file change, for one chip.
 */
function GitChip({ repo }: { repo: string }) {
    const res = useResource(gitOverviewR, repo);
    const st = res.data?.status;
    if (!st) return null;

    const dirty = st.files.length > 0;
    const ahead = st.ahead;
    const behind = st.behind;
    const title = `${st.branch}${st.upstream ? ` → ${st.upstream}` : ""}${dirty ? ` · ${st.files.length} changed` : " · clean"}${ahead ? ` · ahead ${ahead}` : ""}${behind ? ` · behind ${behind}` : ""}`;

    return (
        <>
            <span className="tb-git" data-no-window-drag>
                <Tooltip label={title}>
                    <button className="tb-git-chip" onClick={cmd.openGitPane} aria-label={title}>
                        <IconGit size={12} className={`tb-git-ico ${dirty ? "dirty" : "clean"}`} />
                        <span className="tb-git-branch">{st.branch}</span>
                        {(ahead > 0 || behind > 0) && (
                            <span className="tb-git-track">
                                {ahead > 0 && <span className="tb-git-ahead">↑{ahead}</span>}
                                {behind > 0 && <span className="tb-git-behind">↓{behind}</span>}
                            </span>
                        )}
                    </button>
                </Tooltip>
            </span>
            <span className="tb-sep" />
        </>
    );
}

function CogIcon({ size = 15 }: { size?: number }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width={size}
            height={size}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
    );
}

export function VersionChip() {
    const [version, setVersion] = useState<string | null>(null);
    useEffect(() => {
        getVersion().then(setVersion).catch(swallow("getVersion"));
    }, []);
    if (!version) return null;
    return (
        <Tooltip label={`Sikemux ${version}`}>
            <span className="tb-version">v{version}</span>
        </Tooltip>
    );
}

export function UpdateChip() {
    const pending = useStore((s) => s.pendingUpdate);
    if (!pending) return null;

    const state = pending.state;
    const busy = isUpdateBusy(state);
    const statusLabel = updateStatusLabel(pending);
    const percent = state === "downloading" ? updateDownloadPercent(pending) : null;
    const onClick = () => {
        if (busy) return;
        cmd.openWhatsNew();
    };

    return (
        <Tooltip
            label={
                state === "error"
                    ? `Update v${pending.version} failed — ${pending.error ?? "unknown"}. Click to retry.`
                    : busy
                      ? `${statusLabel} v${pending.version}`
                      : `Update v${pending.version} available (current: v${pending.currentVersion}). Click to install + relaunch.${pending.notes ? `\n\n${pending.notes}` : ""}`
            }>
            <button className={`tb-update tb-update-${state}${percent === null ? "" : " tb-update-measured"}`} onClick={onClick} disabled={busy}>
                {percent !== null && <span className="tb-update-fill" style={{ transform: `scaleX(${percent / 100})` }} aria-hidden="true" />}
                <UpdateArrow size={12} />
                <span className="tb-update-label">{statusLabel}</span>
            </button>
        </Tooltip>
    );
}

function UpdateArrow({ size = 12 }: { size?: number }) {
    return (
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width={size}
            height={size}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true">
            <path d="M8 2v9M4 7l4 4 4-4M3 14h10" />
        </svg>
    );
}

function BatteryChip() {
    const batt = useBattery();
    if (!batt || batt.percent == null) return null;
    const pct = batt.percent;
    const tone = pct <= 10 ? "danger" : pct <= 20 ? "warn" : "ok";
    return (
        <Tooltip label={batt.time_remaining ? `${pct}% · ${batt.time_remaining} remaining` : `${pct}%${batt.charging ? " · charging" : ""}`}>
            <span className={`tb-batt tb-batt-${tone}${batt.charging ? " charging" : ""}`}>
                <IconBattery size={11} percent={pct} charging={batt.charging} />
                <span className="tb-batt-pct">{pct}%</span>
            </span>
        </Tooltip>
    );
}

function ClockChip() {
    const now = useClock();
    const t = twelveHour(now);
    return (
        <span className="tb-clock">
            {t.h}:{time2(t.m)}
            <span className="tb-ampm">{t.ap}</span>
        </span>
    );
}

export const TopBar = memo(function TopBar() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const win = useStore((s) => (session ? s.windows[session.activeWindowId] : undefined));
    const agent = useStore((s) => {
        const id = activeAgentId(s, session);
        return id ? s.agents[id] : undefined;
    });
    const zoomed = useStore((s) => s.zoomedPaneId != null);
    const zen = useStore((s) => s.zenMode);
    const sideRailVisible = useStore((s) => s.sideRailOpen && !s.zenMode);
    const agentRailVisible = useStore((s) => s.agentRailOpen && !s.zenMode);
    const [stripHovered, setStripHovered] = useState(false);
    const plugins = useInstalledPlugins();

    const isProject = !!session && session.kind === "project";
    if (!session || !win) return null;

    return (
        <header className="top-bar" onMouseDown={startWindowDragFromTopBar}>
            <div className="tb-left" />

            <div className="tb-center">
                <div className="crumb">
                    <span className="crumb-kind">{isProject ? <IconFolder size={12} /> : <IconCommand size={12} />}</span>
                    <span className="crumb-session">{session.name}</span>
                    {isProject && (
                        <>
                            <IconChevron size={11} className="crumb-sep" />
                            <span className="crumb-win">
                                {win.role === "agent" ? (
                                    <span className="crumb-name">{agent?.title ?? "agent"}</span>
                                ) : (
                                    <>
                                        <WindowIcon role={win.role} size={12} />
                                        <span className="crumb-name">{win.name}</span>
                                    </>
                                )}
                            </span>
                        </>
                    )}
                </div>
            </div>

            <div className="tb-right" onPointerEnter={() => setStripHovered(true)}>
                {zoomed && (
                    <span className="zoom-pill">
                        <IconZoom size={11} />
                        zoom
                    </span>
                )}
                {isProject && session.cwd && <GitChip repo={session.cwd} />}
                {plugins.map(({ id, TopBarItem }) =>
                    TopBarItem ? <TopBarItem key={id} projectCwd={isProject ? session.cwd || null : null} stripHovered={stripHovered} /> : null,
                )}
                <BatteryChip />
                <ClockChip />
                <div className="tb-toggles">
                    <Tooltip label="Focus mode — hide rails">
                        <button className={`tb-btn${zen ? " on" : ""}`} onClick={cmd.toggleZen} aria-pressed={zen} aria-label="Focus mode">
                            <IconFocus size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip label="Toggle sessions rail">
                        <button
                            className={`tb-btn${sideRailVisible ? " on" : ""}`}
                            onClick={cmd.toggleSideRail}
                            aria-pressed={sideRailVisible}
                            aria-label="Toggle sessions rail">
                            <IconPanelLeft size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip label="Toggle agents rail">
                        <button
                            className={`tb-btn${agentRailVisible ? " on" : ""}`}
                            onClick={cmd.toggleAgentRail}
                            aria-pressed={agentRailVisible}
                            aria-label="Toggle agents rail">
                            <IconAgent size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip label={`Settings — ${PRIMARY_SHORTCUT},`}>
                        <button className="tb-btn" onClick={cmd.toggleSettings} aria-label="Settings">
                            <CogIcon size={15} />
                        </button>
                    </Tooltip>
                </div>
            </div>
        </header>
    );
});
