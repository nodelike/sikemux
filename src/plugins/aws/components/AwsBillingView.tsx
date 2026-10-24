import { useResourceEnabled } from "../../../plugin-api/resources";
import { billingV } from "../resources";
import { selectAws, useAws } from "../state";
import type { AwsBillingChange, AwsBillingMonth } from "../types.generated";
import { ConsoleButton, Crumbs, Header, Inspector, InspectorHead, ListState, Section, Tag, useRegion, useReportCount } from "./parts";

/** The tallest bar stops short of the top so its amount label still fits above it. */
const BAR_REACH = 0.86;
const COST_EXPLORER = "https://us-east-1.console.aws.amazon.com/costmanagement/home#/cost-explorer";

function money(amount: number, unit: string, digits = 2): string {
    const n = Math.abs(amount) < 0.005 ? 0 : amount;
    const symbol = unit === "USD" ? "$" : `${unit} `;
    const body = Math.abs(n).toLocaleString("en-US", { minimumFractionDigits: digits, maximumFractionDigits: digits });
    return `${n < 0 ? "-" : ""}${symbol}${body}`;
}

function short(amount: number, unit: string): string {
    return amount >= 1000 ? `${money(amount / 1000, unit, 1)}k` : money(amount, unit, 0);
}

export function AwsBillingView({ paneId, profile, active }: { paneId: string; profile: string; active: boolean }) {
    const region = useRegion(profile);
    const handle = useResourceEnabled(active, billingV, paneId, profile);
    const picked = useAws((s) => s.selection[profile]?.month);
    const billing = handle.data;
    const current = billing?.months[billing.current];
    useReportCount(profile, "billing", current && billing ? short(current.gross, billing.unit) : undefined);

    const header = (
        <Header
            crumbs={<Crumbs profile={profile} region={region} trail={[{ label: "Billing" }]} />}
            title="Costs"
            tools={<ConsoleButton url={COST_EXPLORER} />}
            handle={handle}
        />
    );
    if (!billing || !current)
        return (
            <div className="aws-main">
                {header}
                <ListState handle={handle} loading="Loading costs…" empty="No billing data for this account." count={billing?.months.length} />
            </div>
        );

    const { months, unit } = billing;
    const selected = months.find((m) => m.periodStart === picked) ?? current;
    const lastClosed = billing.lastClosed === null ? undefined : months[billing.lastClosed];
    const closed = months.filter((m) => !m.inProgress);

    return (
        <>
            <div className="aws-main">
                {header}
                <div className="aws-bill">
                    <div className="aws-bill-hero">
                        <div className="aws-tile big">
                            <div className="aws-tile-label">
                                {current.inProgress ? `${current.name} so far` : current.longLabel}
                                {current.day !== null && (
                                    <span>
                                        day {current.day} of {current.days}
                                    </span>
                                )}
                            </div>
                            <div className="aws-tile-value">{money(current.gross, unit)}</div>
                            {current.day !== null && (
                                <div className="aws-progress">
                                    <b style={{ width: `${(current.day / current.days) * 100}%` }} />
                                </div>
                            )}
                        </div>
                        <div className="aws-tile">
                            <div className="aws-tile-label">Forecast</div>
                            <div className="aws-tile-value">{money(billing.forecast, unit, 0)}</div>
                            {billing.changePct !== null && lastClosed && (
                                <div className={`aws-tile-note ${billing.changePct > 0 ? "up" : "down"}`}>
                                    {billing.changePct > 0 ? "+" : ""}
                                    {billing.changePct.toFixed(1)}% vs {lastClosed.name}
                                </div>
                            )}
                        </div>
                        {lastClosed && (
                            <div className="aws-tile">
                                <div className="aws-tile-label">{lastClosed.name}</div>
                                <div className="aws-tile-value">{money(lastClosed.gross, unit, 0)}</div>
                                <div className="aws-tile-note">closed</div>
                            </div>
                        )}
                        {billing.average !== null && lastClosed && (
                            <div className="aws-tile">
                                <div className="aws-tile-label">{closed.length}-month average</div>
                                <div className="aws-tile-value">{money(billing.average, unit, 0)}</div>
                                <div className="aws-tile-note">
                                    {closed[0].label} – {lastClosed.label}
                                </div>
                            </div>
                        )}
                    </div>
                    <div className="aws-chart">
                        <div className="aws-chart-head">
                            <span>Gross charges by month</span>
                            {billing.average !== null && (
                                <span className="aws-chart-key">
                                    <i />
                                    Average {short(billing.average, unit)}
                                </span>
                            )}
                        </div>
                        <div
                            className="aws-chart-plot"
                            style={billing.averageRatio !== null ? { ["--avg-ratio" as string]: billing.averageRatio * BAR_REACH } : undefined}>
                            {months.map((m) => (
                                <button
                                    key={m.periodStart}
                                    className={`aws-chart-bar${m === selected ? " on" : ""}`}
                                    onClick={() => selectAws(profile, "month", m.periodStart)}
                                    aria-pressed={m === selected}
                                    aria-label={`${m.longLabel}: ${money(m.gross, unit)}`}>
                                    <span className="aws-chart-amount">{short(m.gross, unit)}</span>
                                    <span
                                        className={`aws-chart-col${m.inProgress ? " partial" : ""}`}
                                        style={{ height: `${m.ratio * BAR_REACH * 100}%` }}
                                    />
                                </button>
                            ))}
                        </div>
                        <div className="aws-chart-labels">
                            {months.map((m) => (
                                <span key={m.periodStart} className={m === selected ? "on" : undefined}>
                                    {m.label}
                                    {m.inProgress && <Tag>MTD</Tag>}
                                </span>
                            ))}
                        </div>
                    </div>
                </div>
            </div>
            <MonthInspector month={selected} unit={unit} />
        </>
    );
}

function Change({ change }: { change: AwsBillingChange | null }) {
    if (!change) return <span />;
    if (change.kind === "new") return <span className="aws-bill-change new">new</span>;
    if (change.kind === "flat") return <span className="aws-bill-change">—</span>;
    return (
        <span className={`aws-bill-change ${change.kind}`}>
            {change.pct > 0 ? "+" : ""}
            {change.pct.toFixed(0)}%
        </span>
    );
}

function MonthInspector({ month, unit }: { month: AwsBillingMonth; unit: string }) {
    return (
        <Inspector>
            <InspectorHead title={month.longLabel} badge={month.inProgress ? <Tag>month to date</Tag> : undefined} />
            <div className="aws-bill-total">
                <span className="aws-tile-value">{money(month.gross, unit)}</span>
                {month.credits < -0.005 && (
                    <span className="aws-bill-net">
                        {money(month.credits, unit)} credits · <b>{money(month.net, unit)} net</b>
                    </span>
                )}
            </div>
            <Section title="By service" meta={month.comparedTo ? `${month.inProgress ? "on pace " : ""}vs ${month.comparedTo}` : undefined}>
                <div className="aws-bill-list">
                    {month.charges.length === 0 && <div className="aws-insp-note">No charges this month.</div>}
                    {month.charges.map((s) => (
                        <div key={s.service} className="aws-bill-row">
                            <span className="aws-bill-name" title={s.service}>
                                {s.shortName}
                            </span>
                            <Change change={s.change} />
                            <span className="aws-bill-amount">{money(s.amount, unit)}</span>
                            <span className="aws-bill-bar">
                                <b style={{ width: `${s.ratio * 100}%` }} />
                            </span>
                        </div>
                    ))}
                    {month.creditLines.map((s) => (
                        <div key={s.service} className="aws-bill-row credit">
                            <span className="aws-bill-name" title={s.service}>
                                {s.service}
                            </span>
                            <span />
                            <span className="aws-bill-amount">{money(s.amount, unit)}</span>
                        </div>
                    ))}
                    <div className="aws-bill-row total">
                        <span className="aws-bill-name">{month.creditLines.length ? "Net" : "Total"}</span>
                        <span />
                        <span className="aws-bill-amount">{money(month.net, unit)}</span>
                    </div>
                </div>
            </Section>
        </Inspector>
    );
}
