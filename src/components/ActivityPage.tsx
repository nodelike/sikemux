import { useEffect, useMemo, useState } from "react";
import { activityApi, type ActivityDay, type ActivityShare, type ActivitySummary, type ActivityTotals } from "../api/activity";
import { calendarColumns, dayDate, levelOf, levelThresholds, localDay, streaks } from "../lib/activityCalendar";
import { basename, prettyPath } from "../lib/paths";
import { useStore } from "../state/store";
import type { AgentType } from "../state/types";
import { AgentIcon, IconFolder } from "./Icons";
import { SettingsPage, SettingsSection } from "./SettingsLayout";
import "../styles/activity.css";

type Metric = "agentMs" | "sessions" | "tokens" | "commits";

const METRICS: { id: Metric; label: string }[] = [
    { id: "tokens", label: "Tokens" },
    { id: "commits", label: "Commits" },
    { id: "agentMs", label: "Agent time" },
    { id: "sessions", label: "Sessions" },
];

const AGENT_NAMES: Record<AgentType, string> = {
    claude: "Claude",
    codex: "Codex",
    hermes: "Hermes",
    pi: "Pi",
    opencode: "OpenCode",
    omp: "Oh My Pi",
    grok: "Grok",
};

// Token counts read as K, M and B everywhere; some locales would write lakh and crore.
const COMPACT = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const WHOLE = new Intl.NumberFormat();
const LONG_DATE = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
const SINCE_DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const WEEKDAY_LABELS = ["Mon", "", "Wed", "", "Fri", "", ""];
const NO_DAYS: ActivityDay[] = [];

function duration(ms: number): string {
    const minutes = Math.round(ms / 60_000);
    if (minutes < 60) return `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    if (hours >= 100) return `${WHOLE.format(hours)}h`;
    const rest = minutes % 60;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

function plural(count: number, word: string): string {
    return `${WHOLE.format(count)} ${word}${count === 1 ? "" : "s"}`;
}

function metricText(metric: Metric, value: number): string {
    if (metric === "agentMs") return value > 0 ? `${duration(value)} of agent time` : "No agent time";
    if (metric === "tokens") return `${value > 0 ? COMPACT.format(value) : "No"} tokens`;
    if (metric === "sessions") return value > 0 ? plural(value, "session") : "No sessions";
    return value > 0 ? plural(value, "commit") : "No commits";
}

export function ActivityPage() {
    const [summary, setSummary] = useState<ActivitySummary | null>(null);
    const [failed, setFailed] = useState(false);

    useEffect(() => {
        let live = true;
        activityApi
            .summary()
            .then((next) => live && setSummary(next))
            .catch(() => live && setFailed(true));
        return () => {
            live = false;
        };
    }, []);

    const totals = summary?.totals;
    const empty = !!totals && totals.sessions === 0 && totals.turns === 0 && totals.commits === 0;
    return (
        <SettingsPage>
            <SettingsSection
                title="Overview"
                meta={totals?.firstAtMs != null ? `since ${SINCE_DATE.format(new Date(totals.firstAtMs))}` : undefined}
                sub="Counted from agents run and commits made inside Sikemux. Work done elsewhere is never read.">
                {failed ? (
                    <div className="settings-empty">Activity could not be read.</div>
                ) : empty ? (
                    <div className="settings-empty">Nothing yet. Start an agent and its sessions, turns, tokens and commits collect here.</div>
                ) : (
                    <Overview totals={totals} />
                )}
            </SettingsSection>
            <SettingsSection title="Calendar">
                <Calendar days={summary?.days ?? NO_DAYS} loaded={!!summary} />
            </SettingsSection>
            <SettingsSection title="By agent">
                <ShareList shares={summary?.agents} kind="agent" />
            </SettingsSection>
            <SettingsSection title="By project">
                <ShareList shares={summary?.projects.slice(0, 8)} kind="project" />
            </SettingsSection>
        </SettingsPage>
    );
}

function Overview({ totals }: { totals?: ActivityTotals }) {
    if (!totals) {
        return (
            <div className="activity-stats" aria-busy="true">
                {["Agent time", "Sessions", "Tokens", "Commits"].map((label) => (
                    <Stat key={label} label={label} value="—" detail=" " />
                ))}
            </div>
        );
    }
    const tokens = totals.input + totals.cacheWrite + totals.output;
    const started = totals.sessions - totals.resumed;
    return (
        <div className="activity-stats">
            <Stat label="Agent time" value={duration(totals.agentMs)} detail={plural(totals.turns, "turn")} />
            <Stat
                label="Sessions"
                value={WHOLE.format(totals.sessions)}
                detail={`${WHOLE.format(started)} new · ${WHOLE.format(totals.resumed)} resumed`}
            />
            <Stat
                label="Tokens"
                value={COMPACT.format(tokens)}
                detail={`${COMPACT.format(totals.output)} out · ${COMPACT.format(totals.cacheRead)} cached`}
                title={`${WHOLE.format(totals.input)} input, ${WHOLE.format(totals.cacheWrite)} written to cache, ${WHOLE.format(totals.output)} output, ${WHOLE.format(totals.cacheRead)} read from cache`}
            />
            <Stat label="Commits" value={WHOLE.format(totals.commits)} detail={`${WHOLE.format(totals.agentCommits)} by agents`} />
        </div>
    );
}

function Stat({ label, value, detail, title }: { label: string; value: string; detail: string; title?: string }) {
    return (
        <div className="activity-stat" title={title}>
            <span className="activity-stat-label">{label}</span>
            <span className="activity-stat-value">{value}</span>
            <span className="activity-stat-detail">{detail}</span>
        </div>
    );
}

function Calendar({ days, loaded }: { days: ActivityDay[]; loaded: boolean }) {
    const [metric, setMetric] = useState<Metric>(METRICS[0].id);
    const [hovered, setHovered] = useState<number | null>(null);
    const today = localDay();
    const columns = useMemo(() => calendarColumns(today), [today]);
    const byDay = useMemo(() => new Map(days.map((day) => [day.day, day])), [days]);
    const thresholds = useMemo(() => levelThresholds(days.map((day) => day[metric])), [days, metric]);
    const run = useMemo(
        () => streaks(new Set(days.filter((day) => day.sessions + day.agentMs + day.commits + day.tokens > 0).map((day) => day.day)), today),
        [days, today],
    );

    const readout =
        hovered !== null
            ? `${LONG_DATE.format(dayDate(hovered))} · ${metricText(metric, byDay.get(hovered)?.[metric] ?? 0)}`
            : !loaded
              ? ""
              : run.activeDays === 0
                ? "No activity yet"
                : `${plural(run.activeDays, "active day")} · longest streak ${plural(run.longest, "day")}${run.current > 1 ? ` · ${run.current} days running` : ""}`;

    return (
        <div className="activity-calendar">
            <div className="activity-calendar-bar">
                <span className="activity-readout" aria-live="polite">
                    {readout}
                </span>
                <div className="activity-metrics" role="group" aria-label="Calendar shows">
                    {METRICS.map((item) => (
                        <button key={item.id} type="button" aria-pressed={metric === item.id} onClick={() => setMetric(item.id)}>
                            {item.label}
                        </button>
                    ))}
                </div>
            </div>
            <div className="activity-heatmap" onMouseLeave={() => setHovered(null)}>
                <div className="activity-weekdays" aria-hidden="true">
                    {WEEKDAY_LABELS.map((label, index) => (
                        <span key={index}>{label}</span>
                    ))}
                </div>
                <div className="activity-weeks">
                    <div className="activity-months" aria-hidden="true">
                        {columns.map((column, index) => (
                            <span key={index}>{column.month ?? ""}</span>
                        ))}
                    </div>
                    <div
                        className="activity-cells"
                        role="img"
                        aria-label={`Calendar of ${METRICS.find((item) => item.id === metric)?.label.toLowerCase()}`}>
                        {columns.flatMap((column, week) =>
                            column.days.map((day, index) =>
                                day === null ? (
                                    <span key={`${week}-${index}`} className="activity-cell future" />
                                ) : (
                                    <span
                                        key={day}
                                        className={`activity-cell${hovered === day ? " hovered" : ""}`}
                                        data-level={levelOf(byDay.get(day)?.[metric] ?? 0, thresholds)}
                                        onMouseEnter={() => setHovered(day)}
                                    />
                                ),
                            ),
                        )}
                    </div>
                </div>
            </div>
            <div className="activity-legend" aria-hidden="true">
                <span>Less</span>
                {[0, 1, 2, 3, 4].map((level) => (
                    <span key={level} className="activity-cell" data-level={level} />
                ))}
                <span>More</span>
            </div>
        </div>
    );
}

function ShareList({ shares, kind }: { shares?: ActivityShare[]; kind: "agent" | "project" }) {
    const home = useStore((state) => state.home);
    if (!shares) return null;
    if (shares.length === 0) return <div className="settings-empty">Nothing recorded yet.</div>;
    return (
        <div className="activity-shares">
            {shares.map((share) => {
                const agent = kind === "agent" ? (share.name as AgentType) : null;
                const details = [
                    share.sessions > 0 && plural(share.sessions, "session"),
                    share.tokens > 0 && `${COMPACT.format(share.tokens)} tokens`,
                    share.commits > 0 && plural(share.commits, "commit"),
                ].filter(Boolean);
                return (
                    <div key={share.name} className="activity-share">
                        <span className={`activity-share-mark${agent ? ` agent-glyph ${agent}` : ""}`} aria-hidden="true">
                            {agent ? <AgentIcon type={agent} size={20} /> : <IconFolder size={16} />}
                        </span>
                        <span className="activity-share-name">
                            <span className="activity-share-title">{agent ? (AGENT_NAMES[agent] ?? share.name) : basename(share.name)}</span>
                            {!agent && <span className="activity-share-path">{prettyPath(share.name, home)}</span>}
                        </span>
                        <span className="activity-share-details">{details.join(" · ")}</span>
                        <span className="activity-share-value">{share.agentMs > 0 ? duration(share.agentMs) : "—"}</span>
                    </div>
                );
            })}
        </div>
    );
}
