import { useState } from "react";
import { confirmDialog, notify, reportError } from "../../plugin-api/host";
import { hostApi, type PendingApproval, type RepoRef } from "../api";

interface Props {
    repo: RepoRef;
    runId: string;
    /** The deployments the run waits on, as the core read them. */
    pending: readonly PendingApproval[];
}

export function Approvals({ repo, runId, pending }: Props) {
    const [busy, setBusy] = useState(false);
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
