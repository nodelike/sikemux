import { useState } from "react";
import { confirmDialog, notify, reportError } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { hostApi, type RepoRef } from "../api";
import { approvalsR } from "../resources";

/**
 * Only a run GitHub is actually holding has anybody to ask. Every other run
 * would spend a request to be told nothing is waiting.
 */
export function mayBeWaiting(status: string, conclusion: string | null): boolean {
    return status === "waiting" || status === "action_required" || conclusion === "action_required";
}

interface Props {
    repo: RepoRef;
    runId: string;
    status: string;
    conclusion: string | null;
    active: boolean;
}

export function Approvals({ repo, runId, status, conclusion, active }: Props) {
    const found = useResourceEnabled(active && mayBeWaiting(status, conclusion), approvalsR, repo, runId);
    const [busy, setBusy] = useState(false);
    const pending = found.data ?? [];
    if (pending.length === 0) return null;

    const answer = async (state: "approved" | "rejected") => {
        const environments = pending.filter((each) => each.canApprove);
        const names = environments.map((each) => each.environment).join(", ");
        if (state === "rejected") {
            const sure = await confirmDialog({
                title: "Reject this deployment?",
                body: `${names} will not run.`,
                confirmLabel: "Reject",
                destructive: true,
            });
            if (!sure) return;
        }
        setBusy(true);
        try {
            await hostApi(repo.provider).reviewDeployment(
                repo,
                runId,
                environments.map((each) => each.environmentId),
                state,
            );
            notify("success", state === "approved" ? `Approved ${names}` : `Rejected ${names}`);
            invalidate((kind) => kind === "host.approvals" || kind === "host.run" || kind === "host.runs");
        } catch (error) {
            reportError("Could not answer the deployment")(error);
        } finally {
            setBusy(false);
        }
    };

    const mine = pending.some((each) => each.canApprove);
    return (
        <div className="gha-callout" data-tone="warn">
            <div className="gha-approval-text">
                <strong>Waiting for approval</strong>
                <span className="gha-dim">
                    {pending.map((each) => each.environment).join(", ")}
                    {pending.some((each) => each.waitMinutes > 0) && ` · ${Math.max(...pending.map((each) => each.waitMinutes))}m wait`}
                </span>
                {!mine && (
                    <span className="gha-dim">{pending.flatMap((each) => each.reviewers).join(", ") || "Someone else has to sign this off."}</span>
                )}
            </div>
            {mine && (
                <div className="gha-approval-actions">
                    <button type="button" className="gha-btn danger" disabled={busy} onClick={() => void answer("rejected")}>
                        Reject
                    </button>
                    <button type="button" className="gha-btn primary" disabled={busy} onClick={() => void answer("approved")}>
                        Approve
                    </button>
                </div>
            )}
        </div>
    );
}
