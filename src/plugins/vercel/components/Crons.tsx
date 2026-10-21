import { copyWithNotice } from "../../../plugin-api/host";
import { formatAgo } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import type { VercelProject } from "../api";
import { describeCron } from "../cron";
import { vercelCronsR } from "../resources";
import { visit } from "./parts";

/** A project's cron jobs as its production deployment declared them in vercel.json. */
export function Crons({ active, team, project }: { active: boolean; team: string; project: VercelProject }) {
    const crons = useResourceEnabled(active, vercelCronsR, team, project.name);
    if (crons.status === "error" && !crons.data) return <p className="vercel-note">{crons.error}</p>;
    if (!crons.data) return <SkeletonRows rows={3} label="Loading cron jobs" />;
    const { jobs, enabled, updatedAt } = crons.data;
    const host = project.production?.url ?? null;
    if (jobs.length === 0) {
        return (
            <EmptyState
                message={
                    <>
                        No cron jobs. Add a <code>crons</code> list to <code>vercel.json</code>, and they start with the next production deployment.
                    </>
                }
            />
        );
    }
    return (
        <div className="vercel-settings">
            <p className="vercel-note vercel-muted">
                {enabled ? "Running" : "Turned off in Vercel's settings"} · from the production deployment
                {updatedAt ? `, updated ${formatAgo(updatedAt)}` : ""}. Schedules are in UTC.
            </p>
            <div className="vercel-table" role="table" aria-label="Cron jobs">
                {jobs.map((job) => (
                    <div key={`${job.path}:${job.schedule}`} className="vercel-table-row cron" role="row" aria-label={job.path}>
                        <span className={`vercel-dot ${enabled ? "ready" : "canceled"}`} aria-hidden="true" />
                        <span className="vercel-cell vercel-mono">{job.path}</span>
                        <span className="vercel-cell">{describeCron(job.schedule)}</span>
                        <span className="vercel-cell vercel-mono vercel-muted">{job.schedule}</span>
                        <span className="vercel-cell vercel-row-actions">
                            <button type="button" className="vercel-link" onClick={() => copyWithNotice(job.path, "Cron path")}>
                                Copy path
                            </button>
                            {host ? (
                                <button type="button" className="vercel-link" onClick={() => visit(`${host}${job.path}`)}>
                                    Open
                                </button>
                            ) : null}
                        </span>
                    </div>
                ))}
            </div>
        </div>
    );
}
