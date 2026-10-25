import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
    activityApi,
    type ActivityModel,
    type ActivityPricing,
    type ActivityShare,
    type ActivitySummary,
    type ActivityTotals,
    type PriceOverride,
} from "../api/activity";
import { calendarColumns, dayDate, localDay } from "../lib/activityCalendar";
import { basename, prettyPath } from "@shared/lib/paths";
import { agentLabel } from "@shared/agents/catalog";
import { useStore } from "../state/store";
import { AgentIcon, IconFolder } from "@shared/ui/Icons";
import { SettingsPage, SettingsSection } from "../settings/SettingsLayout";
import { onCoreChanged } from "../api/coreChanged";
import { swallow } from "../state/toast";
import "../styles/activity.css";

type Metric = "agentMs" | "sessions" | "tokens" | "commits" | "costUsd";

const METRICS: { id: Metric; label: string }[] = [
    { id: "tokens", label: "Tokens" },
    { id: "costUsd", label: "Cost" },
    { id: "commits", label: "Commits" },
    { id: "agentMs", label: "Agent time" },
    { id: "sessions", label: "Sessions" },
];

// Token counts read as K, M and B everywhere; some locales would write lakh and crore.
const COMPACT = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const WHOLE = new Intl.NumberFormat();
const LONG_DATE = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
const SINCE_DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const WEEKDAY_LABELS = ["Mon", "", "Wed", "", "Fri", "", ""];
const PRICE_DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

function money(usd: number): string {
    const digits = usd >= 1000 ? 0 : 2;
    return new Intl.NumberFormat("en", { style: "currency", currency: "USD", minimumFractionDigits: digits, maximumFractionDigits: digits }).format(
        usd,
    );
}

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

/** Tokens the agents read and wrote, leaving out what they read again from cache. */
export function burntTokens(totals: ActivityTotals): number {
    return totals.input + totals.cacheWrite + totals.output;
}

export function tokensText(count: number): string {
    return COMPACT.format(count);
}

export function dayText(day: number): string {
    return LONG_DATE.format(dayDate(day));
}

function metricText(metric: Metric, value: number): string {
    if (metric === "agentMs") return value > 0 ? `${duration(value)} of agent time` : "No agent time";
    if (metric === "tokens") return `${value > 0 ? COMPACT.format(value) : "No"} tokens`;
    if (metric === "sessions") return value > 0 ? plural(value, "session") : "No sessions";
    if (metric === "costUsd") return value > 0 ? `${money(value)} at API prices` : "No cost";
    return value > 0 ? plural(value, "commit") : "No commits";
}

export function useActivitySummary(project?: string): { summary: ActivitySummary | null; failed: boolean } {
    const [summary, setSummary] = useState<ActivitySummary | null>(null);
    const [failed, setFailed] = useState(false);

    useEffect(() => {
        let live = true;
        const read = () =>
            activityApi
                .summary(project)
                .then((next) => live && setSummary(next))
                .catch(() => live && setFailed(true));
        void read();
        const stop = onCoreChanged("activity", () => void read());
        return () => {
            live = false;
            stop();
        };
    }, [project]);

    return { summary, failed };
}

function isEmptyActivity(totals?: ActivityTotals): boolean {
    return !!totals && totals.sessions === 0 && totals.turns === 0 && totals.commits === 0;
}

export function ActivityPage() {
    const { summary, failed } = useActivitySummary();
    const [metric, setMetric] = useState<Metric>(METRICS[0].id);
    const totals = summary?.totals;
    const empty = isEmptyActivity(totals);
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
                <Calendar summary={summary} metric={metric} onMetric={setMetric} />
            </SettingsSection>
            <SettingsSection title="By model">
                <ModelList models={summary?.models} prices={summary?.pricing.overrides} />
            </SettingsSection>
            <SettingsSection title="By agent">
                <ShareList shares={summary?.agents} kind="agent" metric={metric} />
            </SettingsSection>
            <SettingsSection title="By project">
                <ShareList shares={summary?.projects.slice(0, 8)} kind="project" metric={metric} />
                {summary && <PricingNote pricing={summary.pricing} unpricedTokens={summary.totals.unpricedTokens} />}
            </SettingsSection>
        </SettingsPage>
    );
}

export function Overview({ totals }: { totals?: ActivityTotals }) {
    if (!totals) {
        return (
            <div className="activity-stats" aria-busy="true">
                {["Agent time", "Sessions", "Tokens", "Cost", "Commits"].map((label) => (
                    <Stat key={label} label={label} value="—" detail=" " />
                ))}
            </div>
        );
    }
    const tokens = burntTokens(totals);
    const started = totals.sessions - totals.resumed;
    return (
        <div className="activity-stats">
            <Stat label="Agent time" value={duration(totals.agentMs)} detail={plural(totals.turns, "turn")} />
            <Stat
                label="Sessions"
                value={WHOLE.format(totals.sessions)}
                detail={`${WHOLE.format(totals.resumed)} resumed`}
                title={`${WHOLE.format(started)} new, ${WHOLE.format(totals.resumed)} resumed`}
            />
            <Stat
                label="Tokens"
                value={COMPACT.format(tokens)}
                detail={`${COMPACT.format(totals.cacheRead)} cached`}
                title={`${WHOLE.format(totals.input)} input, ${WHOLE.format(totals.cacheWrite)} written to cache, ${WHOLE.format(totals.output)} output, ${WHOLE.format(totals.cacheRead)} read from cache`}
            />
            <Stat
                label="Cost"
                badge="API est."
                value={totals.costUsd > 0 ? money(totals.costUsd) : "—"}
                detail={totals.cacheSavingsUsd > 0 ? `${money(totals.cacheSavingsUsd)} saved` : "not priced yet"}
                title={`What these tokens would cost at API prices. Subscription plans bill separately.${totals.cacheSavingsUsd > 0 ? ` Reading from the cache saved ${money(totals.cacheSavingsUsd)}.` : ""}`}
            />
            <Stat label="Commits" value={WHOLE.format(totals.commits)} detail={`${WHOLE.format(totals.agentCommits)} by agents`} />
        </div>
    );
}

function Stat({ label, badge, value, detail, title }: { label: string; badge?: string; value: string; detail: string; title?: string }) {
    return (
        <div className="activity-stat" title={title}>
            <span className="activity-stat-label">
                {label}
                {badge && <span className="activity-stat-badge">{badge}</span>}
            </span>
            <span className="activity-stat-value">{value}</span>
            <span className="activity-stat-detail">{detail}</span>
        </div>
    );
}

export function Calendar({
    summary,
    metric: shownMetric,
    onMetric,
    onHover,
    footer,
}: {
    summary: ActivitySummary | null;
    metric?: Metric;
    onMetric?: (metric: Metric) => void;
    /** Takes over the readout and measure choice above the calendar, to say what the hovered day holds elsewhere. */
    onHover?: (day: number | null) => void;
    /** Sits at the start of the legend's row. */
    footer?: ReactNode;
}) {
    const [ownMetric, setOwnMetric] = useState<Metric>(METRICS[0].id);
    const metric = shownMetric ?? ownMetric;
    const setMetric = onMetric ?? setOwnMetric;
    const [hovered, setHoveredDay] = useState<number | null>(null);
    const setHovered = (day: number | null) => {
        setHoveredDay(day);
        onHover?.(day);
    };
    const today = localDay();
    const columns = useMemo(() => calendarColumns(today), [today]);
    const byDay = useMemo(() => new Map((summary?.days ?? []).map((day) => [day.day, day])), [summary]);
    const run = summary?.streaks;

    const readout =
        hovered !== null
            ? `${LONG_DATE.format(dayDate(hovered))} · ${metricText(metric, byDay.get(hovered)?.[metric] ?? 0)}`
            : !run
              ? ""
              : run.activeDays === 0
                ? "No activity yet"
                : `${plural(run.activeDays, "active day")} · longest streak ${plural(run.longest, "day")}${run.current > 1 ? ` · ${run.current} days running` : ""}`;

    return (
        <div className="activity-calendar">
            {!onHover && (
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
            )}
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
                                        data-level={byDay.get(day)?.levels[metric] ?? 0}
                                        onMouseEnter={() => setHovered(day)}
                                    />
                                ),
                            ),
                        )}
                    </div>
                </div>
            </div>
            <div className="activity-legend">
                {footer}
                <span aria-hidden="true">Less</span>
                {[0, 1, 2, 3, 4].map((level) => (
                    <span key={level} className="activity-cell" data-level={level} aria-hidden="true" />
                ))}
                <span aria-hidden="true">More</span>
            </div>
        </div>
    );
}

function shareValue(share: ActivityShare, metric: Metric): string {
    if (metric === "costUsd") return share.costUsd > 0 ? money(share.costUsd) : "—";
    if (metric === "tokens") return share.tokens > 0 ? COMPACT.format(share.tokens) : "—";
    if (metric === "commits") return share.commits > 0 ? WHOLE.format(share.commits) : "—";
    if (metric === "sessions") return share.sessions > 0 ? WHOLE.format(share.sessions) : "—";
    return share.agentMs > 0 ? duration(share.agentMs) : "—";
}

function ShareList({ shares, kind, metric }: { shares?: ActivityShare[]; kind: "agent" | "project"; metric: Metric }) {
    const home = useStore((state) => state.home);
    if (!shares) return null;
    if (shares.length === 0) return <div className="settings-empty">Nothing recorded yet.</div>;
    return (
        <div className="activity-shares">
            {shares.map((share) => {
                const agent = kind === "agent" ? share.name : null;
                const details = [
                    metric !== "sessions" && share.sessions > 0 && plural(share.sessions, "session"),
                    metric !== "tokens" && share.tokens > 0 && `${COMPACT.format(share.tokens)} tokens`,
                    metric !== "commits" && share.commits > 0 && plural(share.commits, "commit"),
                    metric !== "agentMs" && share.agentMs > 0 && duration(share.agentMs),
                ].filter(Boolean);
                return (
                    <div key={share.name} className="activity-share">
                        <span className={`activity-share-mark${agent ? ` agent-glyph ${agent}` : ""}`} aria-hidden="true">
                            {agent ? <AgentIcon type={agent} size={20} /> : <IconFolder size={16} />}
                        </span>
                        <span className="activity-share-name">
                            <span className="activity-share-title">{agent ? agentLabel(agent) : basename(share.name)}</span>
                            {!agent && <span className="activity-share-path">{prettyPath(share.name, home)}</span>}
                        </span>
                        <span className="activity-share-details">{details.join(" · ")}</span>
                        <span className="activity-share-value">{shareValue(share, metric)}</span>
                    </div>
                );
            })}
        </div>
    );
}

/** `claude-opus-5-5` reads as Opus 5.5 and `gpt-5.6-sol` as GPT-5.6 Sol; anything else keeps its id. */
function modelName(model: string): string {
    const bare = (model.split("/").pop() ?? model).replace(/\[.*$/, "");
    const claude = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/.exec(bare);
    if (claude) {
        const family = claude[1].charAt(0).toUpperCase() + claude[1].slice(1);
        return `${family} ${claude[2]}${claude[3] ? `.${claude[3]}` : ""}`;
    }
    const gpt = /^gpt-([\d.]+)(?:-(.+))?$/.exec(bare);
    if (gpt) {
        const variant = gpt[2]
            ?.split("-")
            .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
            .join(" ");
        return `GPT-${gpt[1]}${variant ? ` ${variant}` : ""}`;
    }
    return bare;
}

function percent(part: number, whole: number): number {
    return whole > 0 ? Math.round((part / whole) * 100) : 0;
}

function ModelList({ models, prices = {} }: { models?: ActivityModel[]; prices?: Record<string, PriceOverride> }) {
    const [editing, setEditing] = useState<string | null>(null);
    if (!models) return null;
    if (models.length === 0) return <div className="settings-empty">Nothing recorded yet.</div>;
    return (
        <div className="activity-shares">
            {models.map((model) => {
                const name = modelName(model.model);
                const own = prices[model.model];
                const input = model.input + model.cacheRead + model.cacheWrite;
                const details = [
                    `${COMPACT.format(model.input + model.output + model.cacheWrite)} tokens`,
                    model.cacheRead > 0 && `${percent(model.cacheRead, input)}% cached`,
                    model.fastCostUsd > 0 && `${percent(model.fastCostUsd, model.costUsd)}% fast`,
                ].filter(Boolean);
                return (
                    <div key={model.model} className="activity-model">
                        <div className="activity-share">
                            <span className={`activity-share-mark agent-glyph ${model.agent}`} aria-hidden="true">
                                <AgentIcon type={model.agent} size={20} />
                            </span>
                            <span className="activity-share-name">
                                <span className="activity-share-title">{name}</span>
                                {name !== model.model && <span className="activity-share-path">{model.model}</span>}
                                {own && (
                                    <button type="button" className="activity-price-tag" onClick={() => setEditing(model.model)}>
                                        your price
                                    </button>
                                )}
                            </span>
                            <span className="activity-share-details">{details.join(" · ")}</span>
                            {model.priced ? (
                                <span className="activity-share-value">{money(model.costUsd)}</span>
                            ) : (
                                <button type="button" className="activity-share-value activity-set-price" onClick={() => setEditing(model.model)}>
                                    Set price
                                </button>
                            )}
                        </div>
                        {editing === model.model && <PriceForm model={model.model} price={own} onDone={() => setEditing(null)} />}
                    </div>
                );
            })}
        </div>
    );
}

const PRICE_FIELDS: { key: keyof PriceOverride; label: string; optional?: boolean }[] = [
    { key: "input", label: "Input" },
    { key: "output", label: "Output" },
    { key: "cacheRead", label: "Cache read", optional: true },
    { key: "cacheWrite", label: "Cache write", optional: true },
];

function PriceForm({ model, price, onDone }: { model: string; price?: PriceOverride; onDone: () => void }) {
    const [values, setValues] = useState<Record<keyof PriceOverride, string>>(() => ({
        input: price?.input?.toString() ?? "",
        output: price?.output?.toString() ?? "",
        cacheRead: price?.cacheRead?.toString() ?? "",
        cacheWrite: price?.cacheWrite?.toString() ?? "",
    }));
    const parsed = (key: keyof PriceOverride) => (values[key].trim() === "" ? undefined : Number(values[key]));
    const valid = PRICE_FIELDS.every(({ key, optional }) => {
        const value = parsed(key);
        return value === undefined ? optional : Number.isFinite(value) && value >= 0;
    });
    const save = () => {
        if (!valid) return;
        void activityApi
            .setPrice(model, {
                input: parsed("input")!,
                output: parsed("output")!,
                cacheRead: parsed("cacheRead"),
                cacheWrite: parsed("cacheWrite"),
            })
            .catch(swallow("save the price"));
        onDone();
    };
    return (
        <form
            className="activity-price-form"
            onSubmit={(event) => {
                event.preventDefault();
                save();
            }}>
            <span className="activity-price-form-note">USD per million tokens. Cache prices default to the input price.</span>
            <div className="activity-price-fields">
                {PRICE_FIELDS.map(({ key, label, optional }) => (
                    <label key={key}>
                        <span>{label}</span>
                        <input
                            className="settings-input mono"
                            inputMode="decimal"
                            value={values[key]}
                            placeholder={optional ? "same as input" : "0.00"}
                            onChange={(event) => setValues((current) => ({ ...current, [key]: event.target.value }))}
                        />
                    </label>
                ))}
            </div>
            <div className="activity-price-actions">
                {price && (
                    <button
                        type="button"
                        className="settings-btn"
                        onClick={() => {
                            void activityApi.setPrice(model, null).catch(swallow("save the price"));
                            onDone();
                        }}>
                        Use published price
                    </button>
                )}
                <button type="button" className="settings-btn" onClick={onDone}>
                    Cancel
                </button>
                <button type="submit" className="settings-btn primary" disabled={!valid}>
                    Save
                </button>
            </div>
        </form>
    );
}

function PricingNote({ pricing, unpricedTokens }: { pricing: ActivityPricing; unpricedTokens: number }) {
    const source =
        pricing.fetchedAtMs === null
            ? "Published prices could not be loaded"
            : `Prices from models.dev, ${PRICE_DATE.format(new Date(pricing.fetchedAtMs))}`;
    return (
        <div className="activity-pricing-note">
            <span>What these tokens would cost at API prices. Subscription plans bill separately.</span>
            <span>
                {unpricedTokens > 0 && `${COMPACT.format(unpricedTokens)} tokens have no price · `}
                {source}
            </span>
        </div>
    );
}
