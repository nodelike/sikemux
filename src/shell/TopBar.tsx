import { memo, useState, type MouseEvent as ReactMouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useBattery } from "../hooks/useBattery";
import { useClock } from "../hooks/useClock";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { swallow } from "../state/toast";
import { gitOverviewR } from "../state/resources.defs";
import { useInstalledPlugins } from "../plugins/installed";
import { useStore } from "../state/store";
import { IconBattery, IconFocus, IconGit, IconMic, IconZoom } from "../ui/Icons";
import { WorkspaceTabs } from "../workspace/Workspace";
import { LEAVES_SETTINGS } from "../settings/leaveSettings";
import { useVoice } from "../voice/dictation";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { Tooltip } from "../ui/Tooltip";
import { RollingText } from "../ui/RollingText";
import { remoteRepoR } from "../codehost/project";
import { codeHost } from "../codehost/registry";
import { PortsChip } from "../ports/PortsChip";

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
    const remote = useResource(remoteRepoR, repo).data ?? null;
    const host = remote ? codeHost(remote.provider) : undefined;
    const st = res.data?.status;
    if (!st) return null;

    const dirty = st.files.length > 0;
    const ahead = st.ahead;
    const behind = st.behind;
    const title = `${host && remote ? `${host.name} · ${remote.owner}/${remote.name} · ` : ""}${st.branch}${st.upstream ? ` → ${st.upstream}` : ""}${dirty ? ` · ${st.files.length} changed` : " · clean"}${ahead ? ` · ahead ${ahead}` : ""}${behind ? ` · behind ${behind}` : ""}`;

    return (
        <>
            <span className="tb-git" data-no-window-drag {...LEAVES_SETTINGS}>
                <Tooltip label={title}>
                    <button className="tb-git-chip" onClick={cmd.openGitPane} aria-label={title}>
                        {host ? (
                            <span className="tb-git-host">{host.icon(12)}</span>
                        ) : (
                            <IconGit size={12} className={`tb-git-ico ${dirty ? "dirty" : "clean"}`} />
                        )}
                        <span className="tb-git-branch">{st.branch}</span>
                        {(ahead > 0 || behind > 0) && (
                            <span className="tb-git-track">
                                {ahead > 0 && (
                                    <span className="tb-git-ahead">
                                        ↑<RollingText text={String(ahead)} />
                                    </span>
                                )}
                                {behind > 0 && (
                                    <span className="tb-git-behind">
                                        ↓<RollingText text={String(behind)} />
                                    </span>
                                )}
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

function VoicePill() {
    const phase = useVoice((s) => s.phase);
    if (phase !== "listening" && phase !== "transcribing") return null;
    return (
        <span className={`voice-pill voice-pill-${phase}`} role="status">
            <IconMic size={11} />
            {phase === "listening" ? "listening" : "writing"}
        </span>
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
            <RollingText text={`${t.h}:${time2(t.m)}`} />
            <span className="tb-ampm">{t.ap}</span>
        </span>
    );
}

export const TopBar = memo(function TopBar() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const zoomed = useStore((s) => s.zoomedPaneId != null);
    const zen = useStore((s) => s.zenMode);
    const focusShortcut = useShortcutLabel("view.focusMode");
    const settingsShortcut = useShortcutLabel("settings.toggle");
    const [stripHovered, setStripHovered] = useState(false);
    const plugins = useInstalledPlugins();

    const isProject = !!session && session.kind === "project";
    if (!session) return null;

    return (
        <header className="top-bar" onMouseDown={startWindowDragFromTopBar}>
            <div className="tb-left" />

            <div className="tb-center" {...LEAVES_SETTINGS}>
                <WorkspaceTabs />
            </div>

            <div className="tb-right" onPointerEnter={() => setStripHovered(true)}>
                <VoicePill />
                {zoomed && (
                    <span className="zoom-pill">
                        <IconZoom size={11} />
                        zoom
                    </span>
                )}
                {isProject && session.cwd && <GitChip repo={session.cwd} />}
                {isProject && session.cwd && <PortsChip sessionId={session.id} />}
                {plugins.map(({ id, TopBarItem }) =>
                    TopBarItem ? <TopBarItem key={id} projectCwd={isProject ? session.cwd || null : null} stripHovered={stripHovered} /> : null,
                )}
                <BatteryChip />
                <ClockChip />
                <div className="tb-toggles">
                    <Tooltip label={withShortcut("Focus mode — hide rails", focusShortcut)}>
                        <button className={`tb-btn${zen ? " on" : ""}`} onClick={cmd.toggleZen} aria-pressed={zen} aria-label="Focus mode">
                            <IconFocus size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip label={withShortcut("Settings", settingsShortcut)}>
                        <button className="tb-btn" onClick={cmd.toggleSettings} aria-label="Settings">
                            <CogIcon size={15} />
                        </button>
                    </Tooltip>
                </div>
            </div>
        </header>
    );
});
