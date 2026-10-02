import { memo, useMemo } from "react";
import type { Job, Run } from "../api";
import { stagesOf } from "../jobGraph";
import { elapsedMs, formatDuration, jobsSummary, outcomeOf } from "../runStatus";
import { OutcomeIcon } from "./ActionsIcon";
import { coarse } from "./hooks";

interface Props {
    run: Run;
    jobs: Job[];
    now: number;
    openJob: string | null;
    onOpen: (jobId: string) => void;
    fileShown: boolean;
    /** Left out when the host cannot show the file a run came from. */
    onToggleFile: (() => void) | null;
}

function fileName(path: string | null): string | null {
    if (!path) return null;
    return path.split("/").pop() ?? path;
}

export const JobGraph = memo(function JobGraph({ run, jobs, now, openJob, onOpen, fileShown, onToggleFile }: Props) {
    const stages = useMemo(() => stagesOf(jobs), [jobs]);
    const name = fileName(run.path);
    const summary = jobsSummary(jobs);

    return (
        <section className="gha-graph">
            <div className="gha-graph-head">
                <span className="gha-graph-file">{name ?? run.name}</span>
                {summary.total > 0 && (
                    <span className="gha-dim">
                        {summary.done} of {summary.total} job{summary.total === 1 ? "" : "s"} done
                        {summary.failed > 0 && <span className="gha-failed-count"> · {summary.failed} failed</span>}
                    </span>
                )}
                <span className="gha-graph-spacer" />
                {onToggleFile && run.path && !run.path.startsWith("dynamic/") && (
                    <button type="button" className="gha-link" onClick={onToggleFile}>
                        {fileShown ? "Hide workflow file" : "Workflow file"}
                    </button>
                )}
            </div>
            {stages.length === 0 ? (
                <div className="gha-dim gha-graph-empty">No jobs yet</div>
            ) : (
                <div className="gha-graph-stages">
                    {stages.map((stage, index) => (
                        <div className="gha-graph-stage" key={stage.map((group) => group.key).join("|")} data-first={index === 0 ? "1" : "0"}>
                            {stage.map((group) => (
                                <div className="gha-graph-group" key={group.key} data-titled={group.title ? "1" : "0"}>
                                    {group.title && <div className="gha-graph-group-title">{group.title}</div>}
                                    {group.jobs.map(({ job, label }) => {
                                        const outcome = outcomeOf(job);
                                        const took = elapsedMs(job.startedAt, job.completedAt, job.completedAt ? coarse(now) : now);
                                        return (
                                            <button
                                                key={job.id}
                                                type="button"
                                                className="gha-graph-job"
                                                data-outcome={outcome}
                                                data-on={openJob === job.id ? "1" : "0"}
                                                title={job.name}
                                                onClick={() => onOpen(job.id)}>
                                                <OutcomeIcon outcome={outcome} size={12} />
                                                <span className="gha-graph-job-name">{label}</span>
                                                {took !== null && outcome !== "skipped" && (
                                                    <span className="gha-graph-job-took">{formatDuration(took)}</span>
                                                )}
                                            </button>
                                        );
                                    })}
                                </div>
                            ))}
                        </div>
                    ))}
                </div>
            )}
        </section>
    );
});
