import type { CheckTally, ListedRun, RepoRef } from "../api";
import { elapsedMs, eventLabel, outcomeOf, OUTCOME_LABEL, tallySummary } from "../runStatus";
import { StatusMark } from "./ActionsIcon";
import { useNow } from "../../plugin-api/hooks";
import { useCiWatch } from "../ciWatch";
import { formatDuration } from "@shared/lib/format";

interface Props {
    repo: RepoRef;
    /** The runs on the pull request's head commit, as the core counted them. */
    checks: CheckTally | null;
    runs: readonly ListedRun[];
    active: boolean;
    onOpenRun: (runId: string) => void;
}

/** The checks part of a pull request's merge box, one row per workflow run on its head commit. */
export function PullChecks({ repo, checks, runs, active, onOpenRun }: Props) {
    const now = useNow(active && runs.some((listed) => listed.unfinished));
    useCiWatch(repo, active);
    if (!checks || runs.length === 0) return null;

    const summary = tallySummary(checks);
    return (
        <div className="gha-merge-part">
            <div className="gha-merge-row">
                <StatusMark outcome={checks.outcome} size={14} />
                <span className="gha-merge-title">{summary === "all passed" ? "All checks passed" : `Checks: ${summary}`}</span>
            </div>
            <div className="gha-checks">
                {runs.map(({ run, unfinished }) => {
                    const outcome = outcomeOf(run);
                    return (
                        <button key={run.id} type="button" className="gha-check" data-outcome={outcome} onClick={() => onOpenRun(run.id)}>
                            <StatusMark outcome={outcome} size={13} />
                            <span className="gha-check-name">
                                {run.name} <span className="gha-item-number gha-dim">#{run.runNumber}</span>
                            </span>
                            <span className="gha-dim">{eventLabel(run.event)}</span>
                            <span className="gha-check-spacer" />
                            <span className="gha-dim">{OUTCOME_LABEL[outcome]}</span>
                            <span className="gha-check-took">
                                {formatDuration(elapsedMs(run.startedAt ?? run.createdAt, unfinished ? null : run.updatedAt, now))}
                            </span>
                        </button>
                    );
                })}
            </div>
        </div>
    );
}
