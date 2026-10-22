import { RunChip } from "../tasks/RunChip";
import { useProjectRepo } from "../git/projectRepo";
import { memo, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import type { BuiltPlugin } from "../plugins/enabled";
import { PlaceArrows } from "./PlaceArrows";
import { TopBarPlace } from "./TopBarPlace";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useBattery } from "../hooks/useBattery";
import { useNow } from "../hooks/useNow";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { swallow } from "../state/toast";
import { gitOverviewR } from "../state/resources.defs";
import { useInstalledPlugins } from "../plugins/installed";
import { selectFocusMode } from "../state/selectors";
import { useStore } from "../state/store";
import { IconBattery, IconFocus, IconGit, IconMic } from "../ui/Icons";
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
/** The chip for the repository the project's Git views use, which may sit inside the project folder, with its code host's CI beside it. */
function ProjectGitChip({ project, children }: { project: string; children?: ReactNode }) {
    const { repo } = useProjectRepo(project);
    return repo ? <GitChip repo={repo}>{children}</GitChip> : null;
}

function GitChip({ repo, children }: { repo: string; children?: ReactNode }) {
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
            {children}
            <span className="tb-sep" />
        </>
    );
}

function VoicePill() {
    const phase = useVoice((s) => s.phase);
    if (phase !== "listening" && phase !== "transcribing") return null;
    return (
        <Tooltip label={phase === "listening" ? "Listening" : "Writing what you said"}>
            <span className={`voice-mic ${phase}`} role="status" aria-label={phase === "listening" ? "Listening" : "Writing what you said"}>
                <IconMic size={14} />
            </span>
        </Tooltip>
    );
}

function BatteryChip() {
    const batt = useBattery();
    if (!batt || batt.percent == null) return null;
    const pct = batt.percent;
    const tone = batt.charging ? "ok" : pct <= 10 ? "danger" : pct <= 20 ? "warn" : "ok";
    const detail = batt.charging
        ? batt.time_remaining
            ? `charging · ${batt.time_remaining} until full`
            : "charging"
        : batt.time_remaining && `${batt.time_remaining} remaining`;
    return (
        <Tooltip label={detail ? `${pct}% · ${detail}` : `${pct}%`}>
            <span className={`tb-batt tb-batt-${tone}${batt.charging ? " charging" : ""}`}>
                <IconBattery size={12} percent={pct} charging={batt.charging} />
                <span className="tb-batt-pct">{pct}%</span>
            </span>
        </Tooltip>
    );
}

function ClockChip() {
    const now = new Date(useNow(true, 60_000));
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
    const zen = useStore(selectFocusMode);
    const focusShortcut = useShortcutLabel("view.focusMode");
    const [stripHovered, setStripHovered] = useState(false);
    const plugins = useInstalledPlugins();

    const isProject = !!session && session.kind === "project";
    if (!session) return null;
    const pluginItem = ({ id, TopBarItem }: BuiltPlugin) =>
        TopBarItem ? <TopBarItem key={id} projectCwd={isProject ? session.cwd || null : null} stripHovered={stripHovered} /> : null;

    return (
        <header className="top-bar" onMouseDown={startWindowDragFromTopBar}>
            <div className="tb-left" />
            <div className="tb-grow" />
            <TopBarPlace />

            <div className="tb-right" onPointerEnter={() => setStripHovered(true)}>
                <VoicePill />
                {isProject && session.cwd && (
                    <ProjectGitChip project={session.cwd}>{plugins.filter(({ id }) => codeHost(id)).map(pluginItem)}</ProjectGitChip>
                )}
                {isProject && session.cwd && <RunChip sessionId={session.id} project={session.cwd} />}
                {isProject && session.cwd && <PortsChip sessionId={session.id} />}
                {plugins.filter(({ id }) => !codeHost(id)).map(pluginItem)}
                <BatteryChip />
                <ClockChip />
                <div className="tb-toggles">
                    <Tooltip label={withShortcut("Focus mode — hide rails", focusShortcut)}>
                        <button className={`tb-btn${zen ? " on" : ""}`} onClick={cmd.toggleZen} aria-pressed={zen} aria-label="Focus mode">
                            <IconFocus size={15} />
                        </button>
                    </Tooltip>
                </div>
            </div>
        </header>
    );
});

/** The project's tabs on a row of their own under the top bar, edge to edge; its empty stretch drags the window like the bar does. */
export const TabRow = memo(function TabRow() {
    const hasSession = useStore((s) => !!s.sessions[s.activeSessionId]);
    if (!hasSession) return null;
    return (
        <div className="tab-row" onMouseDown={startWindowDragFromTopBar} {...LEAVES_SETTINGS}>
            <PlaceArrows />
            <WorkspaceTabs />
        </div>
    );
});
