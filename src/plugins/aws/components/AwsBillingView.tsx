import { useResourceEnabled } from "../../../plugin-api/resources";
import type { BillingMonth } from "../api";
import { billingMonthsR } from "../resources";
import { selectAws, useAws } from "../state";
import { ConsoleButton, Crumbs, Header, Inspector, InspectorHead, ListState, Section, Tag, useRegion, useReportCount } from "./parts";

const MONTHS_BACK = 5;
const MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
/** The tallest bar stops short of the top so its amount label still fits above it. */
const BAR_REACH = 0.86;
const COST_EXPLORER = "https://us-east-1.console.aws.amazon.com/costmanagement/home#/cost-explorer";

function monthName(periodStart: string, long = false): string {
    const [y, m] = periodStart.split("-");
    const name = MONTH_NAMES[parseInt(m, 10) - 1];
    if (!name) return periodStart;
    return long ? `${name} ${y}` : `${name.slice(0, 3)} ${y.slice(2)}`;
}

function money(amount: number, unit: string, digits = 2): string {
    const n = Math.abs(amount) < 0.005 ? 0 : amount;
    const symbol = unit === "USD" ? "$" : `${unit} `;
    const body = Math.abs(n).toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
    return `${n < 0 ? "-" : ""}${symbol}${body}`;
}

function short(amount: number, unit: string): string {
    return amount >= 1000 ? `${money(amount / 1000, unit, 1)}k` : money(amount, unit, 0);
}

function split(month: BillingMonth) {
    let gross = 0;
    let credits = 0;
    for (const s of month.by_service) {
        const n = Number(s.amount);
        if (!Number.isFinite(n)) continue;
        if (n >= 0) gross += n;
        else credits += n;
    }
    return { gross, credits, net: gross + credits };
}

/** How far through the month Cost Explorer's day, which is UTC, has got. */
function monthProgress(periodStart: string): { day: number; days: number } {
    const [y, m] = periodStart.split("-").map(Number);
    const days = new Date(Date.UTC(y, m, 0)).getUTCDate();
    return { day: Math.min(days, Math.max(1, new Date().getUTCDate())), days };
}

export function AwsBillingView({ profile, active }: { profile: string; active: boolean }) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, billingMonthsR, profile, MONTHS_BACK);
    const picked = useAws((s) => s.selection[profile]?.month);
    const months = handle.data && [...handle.data].sort((a, b) => a.period_start.localeCompare(b.period_start));
    const current = months?.find((m) => m.is_current) ?? months?.[months.length - 1];
    useReportCount(profile, "billing", current ? short(split(current).gross, current.unit) : undefined);

    const header = (
        <Header
            crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "Billing" }]} />}
            title="Costs"
            tools={<ConsoleButton url={COST_EXPLORER} />}
            handle={handle}
        />
    );
    if (!months?.length || !current)
        return (
            <div className="aws-main">
                {header}
                <ListState handle={handle} loading="Loading costs…" empty="No billing data for this account." count={months?.length} />
            </div>
        );

    const splits = months.map(split);
    const selected = months.find((m) => m.period_start === picked) ?? current;
    const unit = current.unit;
    const cur = split(current);
    const closed = months.filter((m) => !m.is_current);
    const lastClosed = closed[closed.length - 1];
    const average = closed.length ? closed.reduce((sum, m) => sum + split(m).gross, 0) / closed.length : null;
    const { day, days } = monthProgress(current.period_start);
    const forecast = current.is_current ? (cur.gross / day) * days : cur.gross;
    const lastGross = lastClosed ? split(lastClosed).gross : null;
    const change = lastGross ? ((forecast - lastGross) / lastGross) * 100 : null;
    const tallest = Math.max(1, ...splits.map((s) => s.gross));

    return (
        <>
            <div className="aws-main">
                {header}
                <div className="aws-bill">
                    <div className="aws-bill-hero">
                        <div className="aws-tile big">
                            <div className="aws-tile-label">
                                {current.is_current
                                    ? `${MONTH_NAMES[Number(current.period_start.slice(5, 7)) - 1]} so far`
                                    : monthName(current.period_start, true)}
                                {current.is_current && (
                                    <span>
                                        day {day} of {days}
                                    </span>
                                )}
                            </div>
                            <div className="aws-tile-value">{money(cur.gross, unit)}</div>
                            {current.is_current && (
                                <div className="aws-progress">
                                    <b style={{ width: `${(day / days) * 100}%` }} />
                                </div>
                            )}
                        </div>
                        <div className="aws-tile">
                            <div className="aws-tile-label">Forecast</div>
                            <div className="aws-tile-value">{money(forecast, unit, 0)}</div>
                            {change !== null && lastClosed && (
                                <div className={`aws-tile-note ${change > 0 ? "up" : "down"}`}>
                                    {change > 0 ? "+" : ""}
                                    {change.toFixed(1)}% vs {MONTH_NAMES[Number(lastClosed.period_start.slice(5, 7)) - 1]}
                                </div>
                            )}
                        </div>
                        {lastClosed && lastGross !== null && (
                            <div className="aws-tile">
                                <div className="aws-tile-label">{MONTH_NAMES[Number(lastClosed.period_start.slice(5, 7)) - 1]}</div>
                                <div className="aws-tile-value">{money(lastGross, unit, 0)}</div>
                                <div className="aws-tile-note">closed</div>
                            </div>
                        )}
                        {average !== null && (
                            <div className="aws-tile">
                                <div className="aws-tile-label">{closed.length}-month average</div>
                                <div className="aws-tile-value">{money(average, unit, 0)}</div>
                                <div className="aws-tile-note">
                                    {monthName(closed[0].period_start)} – {monthName(lastClosed.period_start)}
                                </div>
                            </div>
                        )}
                    </div>
                    <div className="aws-chart">
                        <div className="aws-chart-head">
                            <span>Gross charges by month</span>
                            {average !== null && (
                                <span className="aws-chart-key">
                                    <i />
                                    Average {short(average, unit)}
                                </span>
                            )}
                        </div>
                        <div
                            className="aws-chart-plot"
                            style={average !== null ? { ["--avg-ratio" as string]: (average / tallest) * BAR_REACH } : undefined}>
                            {months.map((m, i) => (
                                <button
                                    key={m.period_start}
                                    className={`aws-chart-bar${m === selected ? " on" : ""}`}
                                    onClick={() => selectAws(profile, "month", m.period_start)}
                                    aria-pressed={m === selected}
                                    aria-label={`${monthName(m.period_start, true)}: ${money(splits[i].gross, m.unit)}`}>
                                    <span className="aws-chart-amount">{short(splits[i].gross, m.unit)}</span>
                                    <span
                                        className={`aws-chart-col${m.is_current ? " partial" : ""}`}
                                        style={{ height: `${(splits[i].gross / tallest) * BAR_REACH * 100}%` }}
                                    />
                                </button>
                            ))}
                        </div>
                        <div className="aws-chart-labels">
                            {months.map((m) => (
                                <span key={m.period_start} className={m === selected ? "on" : undefined}>
                                    {monthName(m.period_start)}
                                    {m.is_current && <Tag>MTD</Tag>}
                                </span>
                            ))}
                        </div>
                    </div>
                </div>
            </div>
            <MonthInspector month={selected} previous={months[months.indexOf(selected) - 1]} />
        </>
    );
}

function MonthInspector({ month, previous }: { month: BillingMonth; previous: BillingMonth | undefined }) {
    const totals = split(month);
    const charges = month.by_service.filter((s) => Number(s.amount) > 0.005).sort((a, b) => Number(b.amount) - Number(a.amount));
    const credits = month.by_service.filter((s) => Number(s.amount) < -0.005).sort((a, b) => Number(a.amount) - Number(b.amount));
    const top = Math.max(1, Number(charges[0]?.amount ?? 1));
    const pace = month.is_current
        ? (() => {
              const { day, days } = monthProgress(month.period_start);
              return days / day;
          })()
        : 1;
    const change = (service: string, amount: number) => {
        const before = Number(previous?.by_service.find((s) => s.service === service)?.amount);
        if (!previous) return null;
        if (!Number.isFinite(before) || before <= 0.005) return <span className="aws-bill-change new">new</span>;
        const pct = ((amount * pace - before) / before) * 100;
        if (Math.abs(pct) < 1) return <span className="aws-bill-change">—</span>;
        return (
            <span className={`aws-bill-change ${pct > 0 ? "up" : "down"}`}>
                {pct > 0 ? "+" : ""}
                {pct.toFixed(0)}%
            </span>
        );
    };
    return (
        <Inspector>
            <InspectorHead title={monthName(month.period_start, true)} badge={month.is_current ? <Tag>month to date</Tag> : undefined} />
            <div className="aws-bill-total">
                <span className="aws-tile-value">{money(totals.gross, month.unit)}</span>
                {totals.credits < -0.005 && (
                    <span className="aws-bill-net">
                        {money(totals.credits, month.unit)} credits · <b>{money(totals.net, month.unit)} net</b>
                    </span>
                )}
            </div>
            <Section title="By service" meta={previous ? `${month.is_current ? "on pace " : ""}vs ${monthName(previous.period_start)}` : undefined}>
                <div className="aws-bill-list">
                    {charges.length === 0 && <div className="aws-insp-note">No charges this month.</div>}
                    {charges.map((s) => (
                        <div key={s.service} className="aws-bill-row">
                            <span className="aws-bill-name" title={s.service}>
                                {s.service.replace(/^(Amazon|AWS) /, "")}
                            </span>
                            {change(s.service, Number(s.amount)) ?? <span />}
                            <span className="aws-bill-amount">{money(Number(s.amount), s.unit)}</span>
                            <span className="aws-bill-bar">
                                <b style={{ width: `${(Number(s.amount) / top) * 100}%` }} />
                            </span>
                        </div>
                    ))}
                    {credits.map((s) => (
                        <div key={s.service} className="aws-bill-row credit">
                            <span className="aws-bill-name" title={s.service}>
                                {s.service}
                            </span>
                            <span />
                            <span className="aws-bill-amount">{money(Number(s.amount), s.unit)}</span>
                        </div>
                    ))}
                    <div className="aws-bill-row total">
                        <span className="aws-bill-name">{credits.length ? "Net" : "Total"}</span>
                        <span />
                        <span className="aws-bill-amount">{money(totals.net, month.unit)}</span>
                    </div>
                </div>
            </Section>
        </Inspector>
    );
}
