import { useResourceEnabled } from "../../../plugin-api/resources";
import { gcpApi, type SchedulerJob } from "../api";
import { schedulerJobsR } from "../resources";
import { useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, keyOf, Loaded, PageHeader, Status, Table, useAction, visit } from "./parts";

const targetLabel = (job: SchedulerJob) =>
    !job.target
        ? "—"
        : job.target.kind === "http"
          ? `${job.target.method} ${job.target.uri.replace(/^https:\/\//, "")}`
          : job.target.kind === "pubsub"
            ? `Pub/Sub: ${job.target.topic}`
            : `App Engine: ${job.target.uri}`;

export function SchedulerPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const jobs = useResourceEnabled(active, schedulerJobsR, ...keyOf(target));
    const action = useAction();
    const words = view.filter.trim().toLowerCase();
    const change = (job: SchedulerJob, verb: "run" | "pause" | "resume") => {
        const words = {
            run: ["Run", `${job.name} runs now, outside its schedule.`, "Running"],
            pause: ["Pause", `${job.name} stops running on its schedule until resumed.`, "Paused"],
            resume: ["Resume", `${job.name} runs on its schedule again.`, "Resumed"],
        }[verb];
        void action.run({ title: `${words[0]} ${job.name}?`, body: words[1], confirmLabel: words[0] }, `${words[2]} ${job.name}`, () =>
            gcpApi.schedulerChange(target, job.location, job.name, verb),
        );
    };
    return (
        <div className="gcp-page">
            <PageHeader title="Cloud Scheduler">
                <Button onClick={() => visit(consoleUrl("cloudscheduler", target.project))}>Create job in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter jobs" />
            <Loaded handle={jobs} label="jobs" empty={(rows) => (rows.length === 0 ? "No Cloud Scheduler jobs in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Scheduler jobs"
                        rows={rows.filter((job) => !words || job.name.toLowerCase().includes(words))}
                        rowKey={(job) => `${job.location}/${job.name}`}
                        columns={[
                            { label: "", width: "24px", cell: (job) => <Status state={job.state} /> },
                            {
                                label: "Name",
                                width: "minmax(140px, 1.2fr)",
                                cell: (job) => <span title={job.description ?? undefined}>{job.name}</span>,
                            },
                            { label: "Frequency", cell: (job) => <span className="gcp-mono">{job.schedule}</span> },
                            { label: "Time zone", cell: (job) => job.timeZone ?? "—" },
                            { label: "Target", width: "minmax(160px, 1.4fr)", cell: (job) => <span className="gcp-muted">{targetLabel(job)}</span> },
                            {
                                label: "Last run",
                                width: "minmax(110px, auto)",
                                cell: (job) =>
                                    job.lastRunAt ? (
                                        <span
                                            className={job.lastRunOk === false ? "gcp-error-text" : undefined}
                                            title={job.lastRunError ?? undefined}>
                                            {ago(job.lastRunAt)}
                                            {job.lastRunOk === false ? " · failed" : ""}
                                        </span>
                                    ) : (
                                        "Never"
                                    ),
                            },
                            {
                                label: "",
                                width: "minmax(160px, auto)",
                                align: "end",
                                cell: (job) => (
                                    <>
                                        <Button disabled={action.busy || job.state !== "ENABLED"} onClick={() => change(job, "run")}>
                                            Force run
                                        </Button>
                                        {job.state === "PAUSED" ? (
                                            <Button disabled={action.busy} onClick={() => change(job, "resume")}>
                                                Resume
                                            </Button>
                                        ) : (
                                            <Button disabled={action.busy} onClick={() => change(job, "pause")}>
                                                Pause
                                            </Button>
                                        )}
                                    </>
                                ),
                            },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}
