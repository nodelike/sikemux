import type { RundeckExecution } from "./api";
import { branchOf, displayStatus, duration, qualifiedName } from "./shape";

export interface RunNotice {
    ok: boolean;
    title: string;
    body: string;
}

const ENDINGS: Record<string, string> = {
    failed: "failed",
    aborted: "was aborted",
    timedout: "timed out",
    "failed-with-retry": "failed and will retry",
};

/** What to tell the person when a run they started ends: a deploy names its branch and where it went. */
export function runNotice(execution: RundeckExecution, branchOptions: string[]): RunNotice {
    const job = execution.job;
    const name = job?.name ?? `Execution #${execution.id}`;
    const place = job?.group || job?.project || execution.project || "";
    const branch = branchOf(job?.options, branchOptions);
    const took = duration(execution["date-started"]?.date ?? null, execution["date-ended"]?.date ?? null);
    const status = (execution.status ?? "").toLowerCase();
    const ok = status === "succeeded";
    const to = place ? ` to ${place}` : "";
    const what = branch ? `Deploy of ${branch}${to}` : `${name}${place ? ` on ${place}` : ""}`;
    const ending = ENDINGS[status] ?? `ended: ${displayStatus(execution.status, execution.customStatus)}`;
    const title = ok ? (branch ? `Deployed ${branch}${to}` : `${name} finished`) : `${what} ${ending}`;
    const body = [branch ? name : null, took && (ok ? `took ${took}` : `after ${took}`)].filter(Boolean).join(" · ");
    return { ok, title, body: body || `#${execution.id}` };
}

/** When Rundeck stopped answering before the run ended, so how it went is unknown. */
export function lostRunNotice(name: string, group: string | null, executionId: number): RunNotice {
    return {
        ok: false,
        title: `Lost track of ${qualifiedName(name, group)}`,
        body: `Rundeck stopped answering before run #${executionId} ended. Check it in Rundeck.`,
    };
}
