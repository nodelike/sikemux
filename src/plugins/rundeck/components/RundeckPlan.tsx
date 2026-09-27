import { IconArrowUp, IconCheck, IconInfo, IconWarning } from "../../../plugin-api/ui";
import type { BranchRelation, PlanResult, PushAction } from "../api";
import { BranchChip } from "./parts";

type Tone = "ok" | "warn" | "danger" | "info";

const RELATION: Record<BranchRelation, { tone: Tone; text: string }> = {
    same: { tone: "ok", text: "Same branch as the live one" },
    "target-contains-deployed": { tone: "ok", text: "Contains everything that is live" },
    "target-missing-deployed": { tone: "danger", text: "Doesn't contain the live branch; this is a different line of work" },
    "unknown-no-deployed-branch": { tone: "info", text: "Nothing deployed successfully yet; this is the first deploy" },
    "unknown-deployed-not-on-origin": { tone: "info", text: "The live branch isn't on origin, so they can't be compared" },
    "unknown-target-not-on-origin": { tone: "warn", text: "Not on origin; Rundeck won't find it when it deploys" },
    "unknown-no-repo": { tone: "info", text: "No local checkout linked, so the branches can't be compared" },
};

const PUSH: Record<PushAction, { tone: Tone; text: string }> = {
    "will-push-current": { tone: "warn", text: "Pushes your checkout before deploying" },
    "will-not-push-different-branch": { tone: "info", text: "Checkout is on another branch; nothing is pushed" },
    "will-not-push-no-repo": { tone: "info", text: "No local checkout linked; nothing is pushed" },
    "will-not-push-detached": { tone: "info", text: "Checkout is on a detached HEAD; nothing is pushed" },
};

interface Props {
    plan: PlanResult | null;
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
                            {plan.deployed_branch ? <BranchChip branch={plan.deployed_branch} /> : <span className="rnd-dim">Nothing yet</span>}
                        </div>
                        <span className="rnd-plan-arrow" aria-hidden="true">
                            →
                        </span>
                        <div className="rnd-plan-side">
                            <span>Will deploy</span>
                            <BranchChip branch={plan.target_branch} />
                        </div>
                    </div>
                    <ul className="rnd-checks">
                        <Check tone={RELATION[plan.branch_relation].tone} text={RELATION[plan.branch_relation].text} />
                        <Check
                            tone={plan.remote_target_exists ? "ok" : "warn"}
                            text={plan.remote_target_exists ? "On origin" : "Not on origin yet"}
                            value={plan.remote_target_exists ? `origin/${plan.target_branch}` : undefined}
                        />
                        {plan.git_root && (
                            <Check
                                tone={plan.current_branch ? "info" : "warn"}
                                text={plan.current_branch ? `Checkout is on ${plan.current_branch}` : "Checkout is on a detached HEAD"}
                                value={plan.head_sha?.slice(0, 7)}
                            />
                        )}
                        {plan.git_root && plan.dirty && <Check tone="warn" text="Checkout has uncommitted changes" value="not deployed" />}
                        {!!plan.ahead && (
                            <Check
                                tone="warn"
                                text={`${plan.ahead} local ${plan.ahead === 1 ? "commit" : "commits"} not pushed`}
                                value={plan.upstream ?? undefined}
                            />
                        )}
                        {!!plan.behind && (
                            <Check
                                tone="info"
                                text={`${plan.behind} ${plan.behind === 1 ? "commit" : "commits"} behind upstream`}
                                value={plan.upstream ?? undefined}
                            />
                        )}
                        <Check tone={PUSH[plan.push_action].tone} text={PUSH[plan.push_action].text} />
                    </ul>
                    {plan.branch_relation_detail && <div className="rnd-card-note">{plan.branch_relation_detail}</div>}
                </>
            )}
        </section>
    );
}

function Check({ tone, text, value }: { tone: Tone; text: string; value?: string }) {
    const icon =
        tone === "ok" ? (
            <IconCheck size={13} />
        ) : tone === "info" ? (
            <IconInfo size={13} />
        ) : tone === "warn" && text.startsWith("Pushes") ? (
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
