import type { BranchRelation, PlanResult, PushAction } from "../api";

const RELATION_BANNER: Record<BranchRelation, { kind: "ok" | "warn" | "danger" | "muted"; text: string }> = {
    same: { kind: "ok", text: "Target matches the deployed branch." },
    "target-contains-deployed": { kind: "ok", text: "Target branch contains the deployed branch." },
    "target-missing-deployed": { kind: "danger", text: "Target does NOT contain the deployed branch — different line of work." },
    "unknown-no-deployed-branch": { kind: "muted", text: "No previous successful deployment — first deploy." },
    "unknown-deployed-not-on-origin": { kind: "muted", text: "Deployed branch isn't on origin — relation unknown." },
    "unknown-target-not-on-origin": { kind: "warn", text: "Target branch isn't on origin — Rundeck won't find it during deploy." },
    "unknown-no-repo": { kind: "muted", text: "No local checkout linked, so the branches can't be compared." },
};

const PUSH_LABEL: Record<PushAction, string> = {
    "will-push-current": "Will push the current branch before deploying.",
    "will-not-push-different-branch": "Local branch differs from target — will not push (Rundeck fetches from origin).",
    "will-not-push-no-repo": "No local checkout linked — nothing to push.",
    "will-not-push-detached": "Local HEAD is detached — will not push.",
};

interface Props {
    plan: PlanResult | null;
    loading: boolean;
    error: string | null;
    branch: string;
    isProd: boolean;
}

export function RundeckPlan({ plan, loading, error, branch, isProd }: Props) {
    const banner = plan ? RELATION_BANNER[plan.branch_relation] : null;
    return (
        <div className="rnd-plan">
            <div className="rnd-plan-head">
                deploy plan
                {loading && <span className="rnd-spinner inline" />}
            </div>
            {!branch && <div className="rnd-plan-row muted">Enter a branch to compute the deploy plan.</div>}
            {branch && loading && !plan && <div className="rnd-plan-row muted">computing plan…</div>}
            {plan && <PlanTable plan={plan} isProd={isProd} />}
            {banner && <div className={`rnd-banner ${banner.kind}`}>{banner.text}</div>}
            {plan?.branch_relation_detail && <div className="rnd-banner muted">{plan.branch_relation_detail}</div>}
            {error && <div className="rnd-banner danger">{error}</div>}
        </div>
    );
}

function currentBranchText(plan: PlanResult): string {
    if (plan.current_branch) return plan.current_branch;
    return plan.git_root ? "detached HEAD" : "—";
}

function PlanTable({ plan, isProd }: { plan: PlanResult; isProd: boolean }) {
    return (
        <div className="rnd-plan-table">
            <PlanRow label="target branch" value={plan.target_branch} />
            <PlanRow label="deployed" value={plan.deployed_branch ?? "—"} />
            <PlanRow label="local repo" value={plan.git_root ?? "none linked"} />
            {plan.git_root && (
                <>
                    <PlanRow label="current branch" value={currentBranchText(plan)} tone={plan.current_branch ? undefined : "warn"} />
                    <PlanRow label="HEAD" value={plan.head_sha ?? (plan.current_branch ? "no commits yet" : "—")} />
                    <PlanRow label="dirty tree" value={plan.dirty ? "yes" : "no"} tone={plan.dirty ? "warn" : "ok"} />
                    <PlanRow label="upstream" value={plan.upstream ?? "—"} />
                    <PlanRow
                        label="ahead / behind"
                        value={plan.ahead != null && plan.behind != null ? `${plan.ahead} ahead, ${plan.behind} behind` : "—"}
                    />
                </>
            )}
            <PlanRow label="on origin" value={plan.remote_target_exists ? "yes" : "no"} tone={plan.remote_target_exists ? "ok" : "warn"} />
            <PlanRow label="push" value={PUSH_LABEL[plan.push_action]} />
            {isProd && <PlanRow label="env" value="production" tone="danger" />}
        </div>
    );
}

function PlanRow({ label, value, tone }: { label: string; value: string; tone?: "ok" | "warn" | "danger" }) {
    return (
        <div className={`rnd-plan-row${tone ? ` tone-${tone}` : ""}`}>
            <span className="rnd-plan-k">{label}</span>
            <span className="rnd-plan-v">{value}</span>
        </div>
    );
}
