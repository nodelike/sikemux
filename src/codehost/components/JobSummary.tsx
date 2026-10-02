import { useResourceEnabled } from "../../plugin-api/resources";
import type { RepoRef } from "../api";
import { jobSummaryR } from "../resources";
import { Prose } from "./Pictures";

interface Props {
    repo: RepoRef;
    checkRunId: string;
    active: boolean;
    jobName: string;
}

export function JobSummary({ repo, checkRunId, active, jobName }: Props) {
    const found = useResourceEnabled(active, jobSummaryR, repo, checkRunId);
    const summary = found.data;
    if (!summary) return null;
    return (
        <details className="gha-summary" open>
            <summary className="gha-summary-head">{jobName} summary</summary>
            <Prose>{summary.body}</Prose>
        </details>
    );
}
