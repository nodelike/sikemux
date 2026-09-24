import type { MatrixCell } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { PRIMARY_SHORTCUT, Tooltip } from "../../../plugin-api/ui";
import { displayStatus, isLiveStatus, relativeTime } from "../shape";
import { BRANCH_GLYPH, branchKind, statusKind } from "./branchStyle";

interface Props {
    paneId: string;
    project: string;
    cell: MatrixCell;
    activeCwd: string | null;
    now: number;
}

export function RundeckMatrixRow({ paneId, project, cell, activeCwd, now }: Props) {
    const { latest, deployed } = cell;
    const deployedBranch = deployed?.branch ?? null;
    const hasBranch = latest?.branch != null || deployedBranch != null;
    const action = hasBranch ? "deploy" : "run";
    const disabled = cell.enabled === false;
    const live = isLiveStatus(latest?.status);
    const latestFailedElsewhere = !!latest && latest.status.toLowerCase() !== "succeeded";
    const kind = branchKind(deployedBranch);
    const when = latest?.ended_at ?? latest?.started_at ?? null;

    const job = (): JobRef => ({
        project,
        jobId: cell.job_id,
        name: cell.name,
        group: cell.group,
        repoPath: cmd.linkedRepoPath({ jobId: cell.job_id, name: cell.name }, activeCwd),
    });

    const open = () => cmd.rundeckPush(paneId, { kind: "service", ...job() });

    const runForm = () => {
        if (disabled) return;
        const ref = job();
        cmd.rundeckPush(paneId, { kind: "service", ...ref });
        cmd.rundeckPush(paneId, { kind: "deploy", ...ref, branch: hasBranch ? (deployedBranch ?? latest?.branch ?? "") : undefined });
    };

    const openLatest = () => {
        if (!latest) return;
        const ref = job();
        cmd.rundeckPush(paneId, { kind: "service", ...ref });
        cmd.rundeckPush(paneId, { kind: "execution", ...ref, executionId: latest.execution_id });
    };

    const hint = [
        `Open ${cell.name}`,
        disabled ? "disabled in Rundeck" : `${PRIMARY_SHORTCUT}click to ${action}`,
        cell.scheduled ? "scheduled" : null,
        cell.error,
    ]
        .filter(Boolean)
        .join(" · ");

    return (
        <div className={`rnd-list-row${disabled ? " disabled" : ""}${live ? " live" : ""}${cell.error ? " has-error" : ""}`}>
            <button
                type="button"
                className="rnd-row-open"
                title={hint}
                aria-label={`Open ${cell.name}`}
                onClick={(event) => {
                    if (event.metaKey || event.ctrlKey) runForm();
                    else open();
                }}>
                <span className={`rnd-row-glyph rnd-branch-${kind}`} aria-hidden="true">
                    {live ? <span className="rnd-live-dot" /> : BRANCH_GLYPH[kind]}
                </span>
                <span className="rnd-row-svc">
                    <span className="rnd-row-name">{cell.name}</span>
                    {cell.scheduled && <span className="rnd-tag">scheduled</span>}
                    {disabled && <span className="rnd-tag muted">disabled</span>}
                    {cell.error && <span className="rnd-tag warn">{cell.error === "timed out" ? "timed out" : "error"}</span>}
                </span>
                <span className="rnd-row-branch">
                    {deployedBranch ? (
                        <span className={`rnd-branch-${kind}`} title={`deployed branch: ${deployedBranch}`}>
                            {deployedBranch}
                        </span>
                    ) : (
                        <span className="rnd-branch-na">—</span>
                    )}
                </span>
                <span className="rnd-row-status">
                    {latest ? (
                        <>
                            <span className={`rnd-status-${statusKind(latest.status)}`}>{displayStatus(latest.status, latest.custom_status)}</span>
                            {latestFailedElsewhere && latest.branch && latest.branch !== deployedBranch && (
                                <span className={`rnd-row-status-branch rnd-branch-${branchKind(latest.branch)}`}> · {latest.branch}</span>
                            )}
                        </>
                    ) : (
                        <span className="rnd-status-unknown">never run</span>
                    )}
                </span>
                <span className="rnd-row-user">{latest?.user ?? "—"}</span>
                <span className="rnd-row-when">{when ? relativeTime(when, now) : "—"}</span>
            </button>
            <span className="rnd-row-actions">
                {latest && (
                    <button className="rnd-row-action" onClick={openLatest} title={`View execution #${latest.execution_id}`}>
                        {live ? "live" : "last"}
                    </button>
                )}
                <Tooltip label={disabled ? "This job is disabled in Rundeck" : `${action} (${PRIMARY_SHORTCUT}click the row)`}>
                    <span className="rnd-row-action-wrap">
                        <button className="rnd-row-action accent" onClick={runForm} disabled={disabled}>
                            {action}
                        </button>
                    </span>
                </Tooltip>
            </span>
        </div>
    );
}
