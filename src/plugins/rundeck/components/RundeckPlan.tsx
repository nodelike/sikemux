import { IconArrowUp, IconCheck, IconInfo, IconWarning } from "../../../plugin-api/ui";
import type { RundeckPlan as Plan, RundeckTone } from "../types.generated";
import { BranchChip } from "./parts";

interface Props {
    plan: Plan | null;
    loading: boolean;
    error: string | null;
    branch: string;
    onRecheck: () => void;
}

export function RundeckPlan({ plan, loading, error, branch, onRecheck }: Props) {
    return (
        <section className="rnd-card">
            <div className="rnd-card-head">
                <b>Plan</b>
                {loading ? (
                    <span className="rnd-plan-busy">
                        <span className="rnd-spinner inline" />
                        Checking…
                    </span>
                ) : (
                    plan && (
                        <button type="button" className="rnd-link" onClick={onRecheck}>
                            Check again
                        </button>
                    )
                )}
            </div>
            {!branch && <div className="rnd-card-note">Pick a branch to see what the deploy will do.</div>}
            {branch && !plan && !error && <div className="rnd-card-note">Working out the plan…</div>}
            {error && <div className="rnd-banner danger">{error}</div>}
            {plan && (
                <>
                    <div className="rnd-plan-flow">
                        <div className="rnd-plan-side">
                            <span>Live now</span>
                            {plan.deployedBranch ? <BranchChip branch={plan.deployedBranch} /> : <span className="rnd-dim">Nothing yet</span>}
                        </div>
                        <span className="rnd-plan-arrow" aria-hidden="true">
                            →
                        </span>
                        <div className="rnd-plan-side">
                            <span>Will deploy</span>
                            <BranchChip branch={plan.targetBranch} />
                        </div>
                    </div>
                    <ul className="rnd-checks">
                        {plan.checks.map((check) => (
                            <Check key={check.text} tone={check.tone} text={check.text} value={check.value ?? undefined} pushes={check.pushes} />
                        ))}
                    </ul>
                    {plan.detail && <div className="rnd-card-note">{plan.detail}</div>}
                </>
            )}
        </section>
    );
}

function Check({ tone, text, value, pushes }: { tone: RundeckTone; text: string; value?: string; pushes: boolean }) {
    const icon =
        tone === "ok" ? (
            <IconCheck size={13} />
        ) : tone === "info" ? (
            <IconInfo size={13} />
        ) : pushes ? (
            <IconArrowUp size={13} />
        ) : (
            <IconWarning size={13} />
        );
    return (
        <li className={`rnd-check ${tone}`}>
            {icon}
            <span>{text}</span>
            {value && <span className="rnd-check-v">{value}</span>}
        </li>
    );
}
