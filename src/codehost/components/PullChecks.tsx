import { useResourceEnabled } from "../../plugin-api/resources";
import type { RepoRef } from "../api";
import { runsR } from "../resources";
import { checksSummary, elapsedMs, eventLabel, formatDuration, isUnfinished, outcomeOf, OUTCOME_LABEL, overallOutcome } from "../runStatus";
import { OutcomeIcon } from "./ActionsIcon";
import { useEvery, useNow } from "./hooks";

const LIVE_REFRESH_MS = 10_000;

interface Props {
    repo: RepoRef;
    sha: string;
    active: boolean;
    onOpenRun: (runId: string) => void;
}

/** The checks part of a pull request's merge box, one row per workflow run on its head commit. */
export function PullChecks({ repo, sha, active, onOpenRun }: Props) {
    const found = useResourceEnabled(active, runsR, { ...repo, headSha: sha, perPage: 30 });
    const runs = found.data?.runs ?? [];
    const live = active && runs.some(isUnfinished);
    const now = useNow(live);
    useEvery(live, LIVE_REFRESH_MS, () => void found.refresh());
    if (runs.length === 0) return null;

    const summary = checksSummary(runs);
    return (
        <div className="gha-merge-part">
            <div className="gha-merge-row">
                <OutcomeIcon outcome={overallOutcome(runs)} size={12} />
                <span className="gha-merge-title">{summary === "all passed" ? "All checks passed" : `Checks: ${summary}`}</span>
            </div>
            <div className="gha-checks">
                {runs.map((run) => {
                    const outcome = outcomeOf(run);
                    const going = isUnfinished(run);
                    return (
                        <button key={run.id} type="button" className="gha-check" data-outcome={outcome} onClick={() => onOpenRun(run.id)}>
                            <OutcomeIcon outcome={outcome} size={11} />
                            <span className="gha-check-name">
                                {run.name} <span className="gha-item-number gha-dim">#{run.runNumber}</span>
                            </span>
                            <span className="gha-dim">{eventLabel(run.event)}</span>
                            <span className="gha-check-spacer" />
                            <span className="gha-dim">{OUTCOME_LABEL[outcome]}</span>
                            <span className="gha-check-took">
                                {formatDuration(elapsedMs(run.startedAt ?? run.createdAt, going ? null : run.updatedAt, now))}
                            </span>
                        </button>
                    );
                })}
            </div>
        </div>
    );
}
