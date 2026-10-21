import { useState } from "react";
import { notify, promptDialog } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { gcpApi, type RunJob, type RunRevision, type RunService, type RunServiceDetail, type Target } from "../api";
import { metricsR, runJobR, runJobsR, runServiceR, runServicesR } from "../resources";
import { splitOpen, updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { LogStream } from "./LogStream";
import { SendToAgent } from "./SendToAgent";
import { serviceForAgent } from "../forAgent";
import { ago, Button, consoleUrl, Copyable, Facts, keyOf, Loaded, PageHeader, Sparkline, Status, Table, Tabs, took, useAction, visit } from "./parts";

const LISTS = [
    { value: "services", label: "Services" },
    { value: "jobs", label: "Jobs" },
] as const;

export function RunPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const list = view.list === "jobs" ? "jobs" : "services";
    const opened = splitOpen(view.open);
    const back = () => updateGcpView(paneId, { open: null, tab: null });
    if (opened && list === "services")
        return <ServicePage paneId={paneId} active={active} target={target} location={opened.location} name={opened.name} onBack={back} />;
    if (opened && list === "jobs") return <JobPage active={active} target={target} location={opened.location} name={opened.name} onBack={back} />;
    return (
        <div className="gcp-page">
            <PageHeader title="Cloud Run">
                <Button onClick={() => visit(consoleUrl("run", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <Tabs label="Cloud Run" tabs={LISTS} value={list} onChange={(value) => updateGcpView(paneId, { list: value, open: null })} />
            <Filter paneId={paneId} placeholder={list === "jobs" ? "Filter jobs" : "Filter services"} />
            {list === "services" ? (
                <ServiceList
                    active={active}
                    target={target}
                    filter={view.filter}
                    onOpen={(service) => updateGcpView(paneId, { open: `${service.location}/${service.name}`, tab: null })}
                />
            ) : (
                <JobList
                    active={active}
                    target={target}
                    filter={view.filter}
                    onOpen={(job) => updateGcpView(paneId, { open: `${job.location}/${job.name}` })}
                />
            )}
        </div>
    );
}

export function Filter({ paneId, placeholder }: { paneId: string; placeholder: string }) {
    const view = useGcpView(paneId);
    return (
        <input
            className="gcp-input gcp-filter-input"
            value={view.filter}
            onChange={(event) => updateGcpView(paneId, { filter: event.target.value })}
            placeholder={placeholder}
            aria-label={placeholder}
            spellCheck={false}
        />
    );
}

const matches = (filter: string, ...fields: (string | null | undefined)[]) => {
    const words = filter.trim().toLowerCase();
    return !words || fields.some((field) => field?.toLowerCase().includes(words));
};

function trafficSummary(service: RunService): string {
    const parts = service.traffic
        .filter((entry) => entry.percent > 0)
        .map((entry) => `${entry.percent}% ${entry.latest ? "latest" : (entry.revision ?? "")}`);
    return parts.join(", ") || "—";
}

function ServiceList({ active, target, filter, onOpen }: { active: boolean; target: Target; filter: string; onOpen: (service: RunService) => void }) {
    const services = useResourceEnabled(active, runServicesR, ...keyOf(target));
    return (
        <Loaded handle={services} label="services" empty={(rows) => (rows.length === 0 ? "No Cloud Run services in this project." : null)}>
            {(rows) => (
                <Table
                    label="Services"
                    rows={rows.filter((service) => matches(filter, service.name, service.location))}
                    rowKey={(service) => `${service.location}/${service.name}`}
                    onOpen={onOpen}
                    columns={[
                        { label: "", width: "24px", cell: (service) => <Status state={service.state} /> },
                        { label: "Name", width: "minmax(140px, 1.2fr)", cell: (service) => <span className="gcp-name">{service.name}</span> },
                        { label: "Region", width: "minmax(100px, 0.7fr)", cell: (service) => service.location },
                        {
                            label: "URL",
                            width: "minmax(160px, 1.6fr)",
                            cell: (service) => <span className="gcp-muted">{service.url?.replace(/^https:\/\//, "") ?? "—"}</span>,
                        },
                        { label: "Traffic", width: "minmax(120px, 1fr)", cell: trafficSummary },
                        {
                            label: "Last deployed",
                            width: "minmax(120px, 0.9fr)",
                            cell: (service) => `${ago(service.updatedAt)}${service.updatedBy ? ` by ${service.updatedBy.split("@")[0]}` : ""}`,
                        },
                    ]}
                />
            )}
        </Loaded>
    );
}

const SERVICE_TABS = [
    { value: "metrics", label: "Metrics" },
    { value: "revisions", label: "Revisions" },
    { value: "logs", label: "Logs" },
    { value: "details", label: "Details" },
] as const;

function ServicePage({
    paneId,
    active,
    target,
    location,
    name,
    onBack,
}: {
    paneId: string;
    active: boolean;
    target: Target;
    location: string;
    name: string;
    onBack: () => void;
}) {
    const view = useGcpView(paneId);
    const tab = SERVICE_TABS.find((each) => each.value === view.tab)?.value ?? "metrics";
    const service = useResourceEnabled(active, runServiceR, ...keyOf(target), location, name);
    const action = useAction();
    const sendAll = (revision: string | null) =>
        void action.run(
            {
                title: revision ? `Send all traffic to ${revision}?` : "Send all traffic to the latest revision?",
                body: revision
                    ? `Every request to ${name} goes to ${revision}, as a rollback does. New deployments will not get traffic until you send it back to the latest.`
                    : `Every request to ${name} goes to its latest ready revision, and to each new one as it is deployed.`,
                confirmLabel: "Send traffic",
            },
            "Traffic is moving",
            () => gcpApi.runSetTraffic(target, location, name, revision),
        );
    const deployImage = async () => {
        const image = await promptDialog({
            title: `Deploy a new revision of ${name}`,
            body: "The image the new revision runs. Every other setting carries over from the current revision.",
            label: "Container image",
            initial: service.data?.image ?? "",
            confirmLabel: "Next",
        });
        if (!image?.trim() || image.trim() === service.data?.image) return;
        await action.run(
            {
                title: `Deploy ${image.trim()}?`,
                body: `${name} gets a new revision running this image, and it takes traffic as the service's traffic settings say.`,
                confirmLabel: "Deploy",
            },
            "Deploying the new revision",
            () => gcpApi.runDeploy(target, location, name, { image: image.trim() }),
        );
    };
    const editScaling = async () => {
        const min = await promptDialog({
            title: `Scaling for ${name}`,
            body: "Instances kept warm even with no traffic. 0 scales to zero.",
            label: "Minimum instances",
            initial: String(service.data?.minInstances ?? 0),
            confirmLabel: "Next",
        });
        if (min === null) return;
        const max = await promptDialog({
            title: `Scaling for ${name}`,
            body: "The most instances it may scale out to.",
            label: "Maximum instances",
            initial: String(service.data?.maxInstances ?? 100),
            confirmLabel: "Next",
        });
        if (max === null) return;
        const [low, high] = [Number(min.trim()), Number(max.trim())];
        if (![low, high].every((count) => Number.isInteger(count) && count >= 0 && count <= 1000) || low > high) {
            notify("error", "Scaling needs whole numbers from 0 to 1000, with the minimum no higher than the maximum");
            return;
        }
        await action.run(
            {
                title: `Scale ${name} to ${low}–${high} instances?`,
                body: "This deploys a new revision with the new scaling; every other setting carries over.",
                confirmLabel: "Deploy",
            },
            "Deploying the new scaling",
            () => gcpApi.runDeploy(target, location, name, { minInstances: low, maxInstances: high }),
        );
    };
    return (
        <div className="gcp-page">
            <PageHeader title={<>Service details: {name}</>} back={{ label: "services", go: onBack }}>
                <Button disabled={action.busy || !service.data} onClick={() => void deployImage()}>
                    Deploy image…
                </Button>
                <Button disabled={action.busy || !service.data} onClick={() => void editScaling()}>
                    Edit scaling…
                </Button>
                <Button disabled={action.busy} onClick={() => sendAll(null)}>
                    Send all traffic to latest
                </Button>
                <SendToAgent disabled={!service.data} delivery={() => (service.data ? serviceForAgent(target.project, service.data) : "")} />
                <Button onClick={() => visit(consoleUrl(`run/detail/${location}/${name}/metrics`, target.project))}>
                    Edit & deploy in Cloud console
                </Button>
            </PageHeader>
            <Loaded handle={service} label={name} empty={() => null}>
                {(detail) => (
                    <>
                        <div className="gcp-summary">
                            <Status state={detail.state} text />
                            <span className="gcp-muted">Region: {detail.location}</span>
                            {detail.url && (
                                <span>
                                    URL: <Copyable text={detail.url} what="the URL" />
                                </span>
                            )}
                            {detail.message && detail.state === "FAILED" && <span className="gcp-error-text">{detail.message}</span>}
                        </div>
                        <Tabs label={name} tabs={SERVICE_TABS} value={tab} onChange={(value) => updateGcpView(paneId, { tab: value })} />
                        {tab === "metrics" && <RunMetrics active={active} target={target} name={name} />}
                        {tab === "revisions" && (
                            <>
                                <TrafficSplit
                                    detail={detail}
                                    busy={action.busy}
                                    onSave={(split) =>
                                        void action.run(
                                            {
                                                title: `Split ${name}'s traffic?`,
                                                body: split
                                                    .map((share) => `${share.percent}% to ${share.revision ?? "the latest revision"}`)
                                                    .join(", "),
                                                confirmLabel: "Save traffic",
                                            },
                                            "Traffic is moving",
                                            () => gcpApi.runSplitTraffic(target, location, name, split),
                                        )
                                    }
                                />
                                <Revisions detail={detail} busy={action.busy} onSendAll={sendAll} />
                            </>
                        )}
                        {tab === "logs" && <LogStream active={active} target={target} query={{ resource: "runService", name }} />}
                        {tab === "details" && <ServiceFacts detail={detail} />}
                    </>
                )}
            </Loaded>
        </div>
    );
}

function RunMetrics({ active, target, name }: { active: boolean; target: Target; name: string }) {
    const [since, setSince] = useState("6h");
    const key = keyOf(target);
    const requests = useResourceEnabled(active, metricsR, ...key, "runRequests", name, since);
    const errors = useResourceEnabled(active, metricsR, ...key, "runErrors", name, since);
    const latency = useResourceEnabled(active, metricsR, ...key, "runLatency", name, since);
    const instances = useResourceEnabled(active, metricsR, ...key, "runInstances", name, since);
    return (
        <>
            <Tabs
                label="Time range"
                tabs={[
                    { value: "1h", label: "1 hour" },
                    { value: "6h", label: "6 hours" },
                    { value: "1d", label: "1 day" },
                    { value: "7d", label: "7 days" },
                ]}
                value={since}
                onChange={setSince}
            />
            <div className="gcp-charts">
                <Sparkline title="Request count" series={requests.data} />
                <Sparkline title="5xx responses" series={errors.data} />
                <Sparkline title="Request latency (p95)" series={latency.data} />
                <Sparkline title="Instances" series={instances.data} />
            </div>
        </>
    );
}

/** The shares a service's traffic splits into now, as the editor starts them. */
export function currentShares(detail: RunServiceDetail): Record<string, number> {
    const shares: Record<string, number> = { "": 0 };
    for (const revision of detail.revisions.filter((each) => each.state === "READY")) shares[revision.name] = 0;
    for (const entry of detail.traffic)
        shares[entry.latest ? "" : (entry.revision ?? "")] = (shares[entry.latest ? "" : (entry.revision ?? "")] ?? 0) + entry.percent;
    return shares;
}

function TrafficSplit({
    detail,
    busy,
    onSave,
}: {
    detail: RunServiceDetail;
    busy: boolean;
    onSave: (split: { revision: string | null; percent: number }[]) => void;
}) {
    const [open, setOpen] = useState(false);
    const [shares, setShares] = useState<Record<string, number>>(() => currentShares(detail));
    const total = Object.values(shares).reduce((sum, share) => sum + share, 0);
    if (!open)
        return (
            <div>
                <Button
                    onClick={() => {
                        setShares(currentShares(detail));
                        setOpen(true);
                    }}>
                    Manage traffic
                </Button>
            </div>
        );
    return (
        <section className="gcp-traffic" aria-label="Manage traffic">
            {Object.keys(shares).map((revision) => (
                <label key={revision || "latest"} className="gcp-traffic-row">
                    <span className="gcp-mono">{revision || "Latest healthy revision"}</span>
                    <input
                        className="gcp-input"
                        type="number"
                        min={0}
                        max={100}
                        value={shares[revision]}
                        onChange={(event) =>
                            setShares({ ...shares, [revision]: Math.max(0, Math.min(100, Math.round(Number(event.target.value) || 0))) })
                        }
                        aria-label={`Traffic to ${revision || "the latest revision"}`}
                    />
                    <span className="gcp-muted">%</span>
                </label>
            ))}
            <div className="gcp-log-bar">
                <span className={total === 100 ? "gcp-muted" : "gcp-error-text"}>Total {total}%</span>
                <Button
                    primary
                    disabled={busy || total !== 100}
                    onClick={() => {
                        onSave(
                            Object.entries(shares)
                                .filter(([, percent]) => percent > 0)
                                .map(([revision, percent]) => ({ revision: revision || null, percent })),
                        );
                        setOpen(false);
                    }}>
                    Save
                </Button>
                <Button onClick={() => setOpen(false)}>Cancel</Button>
            </div>
        </section>
    );
}

function Revisions({ detail, busy, onSendAll }: { detail: RunServiceDetail; busy: boolean; onSendAll: (revision: string) => void }) {
    return (
        <Table
            label="Revisions"
            rows={detail.revisions}
            rowKey={(revision) => revision.name}
            columns={[
                { label: "", width: "24px", cell: (revision: RunRevision) => <Status state={revision.state} /> },
                { label: "Name", width: "minmax(160px, 1.4fr)", cell: (revision) => <span className="gcp-mono">{revision.name}</span> },
                { label: "Traffic", width: "minmax(70px, 0.5fr)", cell: (revision) => (revision.percent > 0 ? `${revision.percent}%` : "—") },
                { label: "Tags", width: "minmax(80px, 0.6fr)", cell: (revision) => revision.tags.join(", ") || "—" },
                { label: "Deployed", width: "minmax(90px, 0.6fr)", cell: (revision) => ago(revision.createdAt) },
                {
                    label: "",
                    width: "minmax(130px, auto)",
                    align: "end",
                    cell: (revision) =>
                        revision.percent === 100 || revision.state !== "READY" ? null : (
                            <Button disabled={busy} onClick={() => onSendAll(revision.name)}>
                                Send all traffic
                            </Button>
                        ),
                },
                { label: "Image", width: "minmax(160px, 1.4fr)", cell: (revision) => <span className="gcp-muted">{revision.image ?? "—"}</span> },
            ]}
        />
    );
}

function ServiceFacts({ detail }: { detail: RunServiceDetail }) {
    return (
        <div className="gcp-section">
            <Facts
                items={[
                    ["Container image", detail.image && <Copyable text={detail.image} what="the image" />],
                    ["CPU", detail.cpu],
                    ["Memory", detail.memory],
                    ["Min instances", detail.minInstances ?? 0],
                    ["Max instances", detail.maxInstances],
                    ["Concurrency", detail.concurrency],
                    ["Ingress", detail.ingress?.replace("INGRESS_TRAFFIC_", "").replace(/_/g, " ").toLowerCase()],
                    ["Service account", detail.serviceAccount],
                    ["Latest revision", detail.latestRevision],
                    ["Last deployed by", detail.updatedBy],
                ]}
            />
            {detail.env.length > 0 && (
                <>
                    <h2 className="gcp-section-title">Environment variables</h2>
                    <p className="gcp-note">Names only. Values stay in Google Cloud.</p>
                    <Table
                        label="Environment variables"
                        rows={detail.env}
                        rowKey={(entry) => entry.name}
                        columns={[
                            { label: "Name", cell: (entry) => <span className="gcp-mono">{entry.name}</span> },
                            { label: "Source", cell: (entry) => (entry.secret ? `Secret Manager: ${entry.secret}` : "Value") },
                        ]}
                    />
                </>
            )}
        </div>
    );
}

function JobList({ active, target, filter, onOpen }: { active: boolean; target: Target; filter: string; onOpen: (job: RunJob) => void }) {
    const jobs = useResourceEnabled(active, runJobsR, ...keyOf(target));
    return (
        <Loaded handle={jobs} label="jobs" empty={(rows) => (rows.length === 0 ? "No Cloud Run jobs in this project." : null)}>
            {(rows) => (
                <Table
                    label="Jobs"
                    rows={rows.filter((job) => matches(filter, job.name, job.location))}
                    rowKey={(job) => `${job.location}/${job.name}`}
                    onOpen={onOpen}
                    columns={[
                        { label: "", width: "24px", cell: (job) => <Status state={job.state} /> },
                        { label: "Name", width: "minmax(140px, 1.2fr)", cell: (job) => <span className="gcp-name">{job.name}</span> },
                        { label: "Region", cell: (job) => job.location },
                        { label: "Executions", cell: (job) => job.executions ?? 0 },
                        { label: "Last executed", cell: (job) => ago(job.latestExecutionAt) },
                        { label: "Image", width: "minmax(160px, 1.4fr)", cell: (job) => <span className="gcp-muted">{job.image ?? "—"}</span> },
                    ]}
                />
            )}
        </Loaded>
    );
}

function JobPage({
    active,
    target,
    location,
    name,
    onBack,
}: {
    active: boolean;
    target: Target;
    location: string;
    name: string;
    onBack: () => void;
}) {
    const job = useResourceEnabled(active, runJobR, ...keyOf(target), location, name);
    const action = useAction();
    const [logs, setLogs] = useState(false);
    return (
        <div className="gcp-page">
            <PageHeader title={<>Job details: {name}</>} back={{ label: "jobs", go: onBack }}>
                <Button
                    primary
                    disabled={action.busy}
                    onClick={() =>
                        void action.run(
                            { title: `Execute ${name}?`, body: `Cloud Run starts ${name}'s tasks now.`, confirmLabel: "Execute" },
                            "The job is starting",
                            () => gcpApi.runJobExecute(target, location, name),
                        )
                    }>
                    Execute
                </Button>
                <Button onClick={() => setLogs(!logs)}>{logs ? "Executions" : "Logs"}</Button>
            </PageHeader>
            <Loaded handle={job} label={name} empty={() => null}>
                {(detail) =>
                    logs ? (
                        <LogStream active={active} target={target} query={{ resource: "runJob", name }} />
                    ) : (
                        <>
                            <Facts
                                items={[
                                    ["Region", detail.location],
                                    ["Tasks per execution", detail.tasks],
                                    ["Image", detail.image],
                                ]}
                            />
                            <Table
                                label="Executions"
                                rows={detail.recent}
                                rowKey={(execution) => execution.name}
                                columns={[
                                    { label: "", width: "24px", cell: (execution) => <Status state={execution.state} /> },
                                    {
                                        label: "Execution",
                                        width: "minmax(160px, 1.4fr)",
                                        cell: (execution) => <span className="gcp-mono">{execution.name}</span>,
                                    },
                                    {
                                        label: "Tasks",
                                        cell: (execution) =>
                                            `${execution.succeeded}/${execution.tasks ?? "?"} done${execution.failed ? `, ${execution.failed} failed` : ""}`,
                                    },
                                    { label: "Started", cell: (execution) => ago(execution.startedAt ?? execution.createdAt) },
                                    { label: "Took", cell: (execution) => took(execution.startedAt, execution.completedAt) },
                                ]}
                            />
                        </>
                    )
                }
            </Loaded>
        </div>
    );
}
