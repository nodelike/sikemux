import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { formatAgo, errorText } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Seg, SkeletonRows } from "../../../plugin-api/ui";
import { vercelApi } from "../api";
import { vercel, vercelFirewallR } from "../resources";

/** How long to keep Attack Challenge Mode on, as the person picks it. */
export const ATTACK_MODE_HOURS = [
    { value: "1", label: "1 hour" },
    { value: "6", label: "6 hours" },
    { value: "24", label: "24 hours" },
    { value: "0", label: "Until turned off" },
];

/** A project's firewall: Attack Challenge Mode, then the rules and IP blocks it applies. */
export function Firewall({ active, team, project }: { active: boolean; team: string; project: string }) {
    const firewall = useResourceEnabled(active, vercelFirewallR, team, project);
    const [busy, setBusy] = useState(false);
    const [hours, setHours] = useState("1");
    if (firewall.status === "error" && !firewall.data) return <p className="vercel-note">{firewall.error}</p>;
    if (!firewall.data) return <SkeletonRows rows={4} label="Loading the firewall" />;
    const { attackMode, attackModeUntil, rules, blocks } = firewall.data;

    const toggle = async () => {
        if (!attackMode) {
            const length = ATTACK_MODE_HOURS.find((each) => each.value === hours)?.label ?? "";
            const when = hours === "0" ? "Until it is turned off" : `For ${length}`;
            const sure = await confirmDialog({
                title: "Turn on Attack Challenge Mode?",
                body: `${when}, every visitor to ${project} passes a browser check before reaching it, and some bots and API clients will be turned away. Use it while the site is under attack.`,
                confirmLabel: "Turn on",
                destructive: true,
            });
            if (!sure) return;
        }
        setBusy(true);
        try {
            await vercelApi.setAttackMode(team, project, !attackMode, attackMode ? undefined : Number(hours) || undefined);
            vercel.refresh(vercelFirewallR);
            notify("success", attackMode ? "Attack Challenge Mode is off" : "Attack Challenge Mode is on");
        } catch (error) {
            notify("error", `Attack Challenge Mode: ${errorText(error)}`);
        } finally {
            setBusy(false);
        }
    };

    return (
        <div className="vercel-settings">
            <section className={`vercel-box vercel-attack${attackMode ? " on" : ""}`} aria-label="Attack Challenge Mode">
                <div className="vercel-domain-head">
                    <span className="vercel-box-title">Attack Challenge Mode</span>
                    {attackMode ? null : <Seg label="For" options={ATTACK_MODE_HOURS} value={hours} onChange={setHours} />}
                    <button type="button" className={`vercel-button${attackMode ? "" : " danger"}`} disabled={busy} onClick={() => void toggle()}>
                        {attackMode ? "Turn off" : "Turn on"}
                    </button>
                </div>
                <span className="vercel-muted">
                    {attackMode
                        ? attackModeUntil
                            ? `On, turning off ${formatAgo(attackModeUntil).replace(/ ago$/, " from now")}. Every visitor passes a browser check first.`
                            : "On until turned off. Every visitor passes a browser check first."
                        : "Off. Turn it on while the site is under attack; every visitor then passes a browser check first."}
                </span>
            </section>
            <section className="vercel-box vercel-form-box" aria-label="Rules">
                <span className="vercel-box-title">Custom Rules</span>
                {rules.length === 0 ? (
                    <EmptyState variant="inline" message="No custom rules. Add them in Vercel's firewall settings." />
                ) : (
                    <div className="vercel-table" role="table" aria-label="Firewall rules">
                        {rules.map((rule) => (
                            <div key={rule.id} className="vercel-table-row rule" role="row" aria-label={rule.name}>
                                <span className={`vercel-dot ${rule.active ? "ready" : "canceled"}`} aria-hidden="true" />
                                <span className="vercel-cell">
                                    {rule.name}
                                    {rule.description ? <span className="vercel-muted"> · {rule.description}</span> : null}
                                </span>
                                <span className="vercel-cell">
                                    <span className="vercel-env">{rule.action}</span>
                                </span>
                                <span className="vercel-cell vercel-muted">{rule.active ? "Active" : "Paused"}</span>
                            </div>
                        ))}
                    </div>
                )}
            </section>
            {blocks.length > 0 ? (
                <section className="vercel-box vercel-form-box" aria-label="IP Blocking">
                    <span className="vercel-box-title">IP Blocking</span>
                    <div className="vercel-table" role="table" aria-label="Blocked addresses">
                        {blocks.map((block) => (
                            <div key={`${block.ip}:${block.hostname ?? ""}`} className="vercel-table-row rule" role="row">
                                <span className="vercel-dot error" aria-hidden="true" />
                                <span className="vercel-cell vercel-mono">{block.ip}</span>
                                <span className="vercel-cell vercel-muted">{block.hostname ?? "Every domain"}</span>
                                <span className="vercel-cell vercel-muted">{block.note ?? ""}</span>
                            </div>
                        ))}
                    </div>
                </section>
            ) : null}
        </div>
    );
}
