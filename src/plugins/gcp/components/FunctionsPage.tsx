import { useResourceEnabled } from "../../../plugin-api/resources";
import type { GcpFunction, Target } from "../api";
import { functionsR, metricsR } from "../resources";
import { splitOpen, updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { LogStream } from "./LogStream";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Copyable, Facts, keyOf, Loaded, PageHeader, Sparkline, Status, Table, Tabs, visit } from "./parts";

const trigger = (fn: GcpFunction) =>
    fn.trigger.kind === "http" ? "HTTP" : fn.trigger.topic ? `Pub/Sub: ${fn.trigger.topic}` : fn.trigger.event.split(".").slice(-2).join(".");

export function FunctionsPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const functions = useResourceEnabled(active, functionsR, ...keyOf(target));
    const opened = splitOpen(view.open);
    const fn = opened && functions.data?.find((each) => each.location === opened.location && each.name === opened.name);
    if (fn)
        return (
            <FunctionPage paneId={paneId} active={active} target={target} fn={fn} onBack={() => updateGcpView(paneId, { open: null, tab: null })} />
        );
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Cloud Functions">
                <Button onClick={() => visit(consoleUrl("functions/list", target.project))}>Create function in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter functions" />
            <Loaded handle={functions} label="functions" empty={(rows) => (rows.length === 0 ? "No functions in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Functions"
                        rows={rows.filter((each) => !words || each.name.toLowerCase().includes(words))}
                        rowKey={(each) => `${each.location}/${each.name}`}
                        onOpen={(each: GcpFunction) => updateGcpView(paneId, { open: `${each.location}/${each.name}`, tab: null })}
                        columns={[
                            { label: "", width: "24px", cell: (each) => <Status state={each.state} /> },
                            { label: "Name", width: "minmax(140px, 1.2fr)", cell: (each) => <span className="gcp-name">{each.name}</span> },
                            { label: "Environment", cell: (each) => each.generation },
                            { label: "Region", cell: (each) => each.location },
                            { label: "Trigger", cell: trigger },
                            { label: "Runtime", cell: (each) => each.runtime ?? "—" },
                            { label: "Last deployed", cell: (each) => ago(each.updatedAt) },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

const TABS = [
    { value: "metrics", label: "Metrics" },
    { value: "details", label: "Details" },
    { value: "logs", label: "Logs" },
] as const;

function FunctionPage({
    paneId,
    active,
    target,
    fn,
    onBack,
}: {
    paneId: string;
    active: boolean;
    target: Target;
    fn: GcpFunction;
    onBack: () => void;
}) {
    const view = useGcpView(paneId);
    const tab = TABS.find((each) => each.value === view.tab)?.value ?? "metrics";
    const executions = useResourceEnabled(active && tab === "metrics", metricsR, ...keyOf(target), "functionExecutions", fn.name, "6h");
    const requests = useResourceEnabled(
        active && tab === "metrics" && !!fn.runService,
        metricsR,
        ...keyOf(target),
        "runRequests",
        fn.runService ?? "",
        "6h",
    );
    return (
        <div className="gcp-page">
            <PageHeader title={<>Function details: {fn.name}</>} back={{ label: "functions", go: onBack }}>
                <Button onClick={() => visit(consoleUrl(`functions/details/${fn.location}/${fn.name}`, target.project))}>
                    Edit in Cloud console
                </Button>
            </PageHeader>
            <div className="gcp-summary">
                <Status state={fn.state} text />
                <span className="gcp-muted">{fn.location}</span>
                <span className="gcp-muted">{fn.generation}</span>
                {fn.url && (
                    <span>
                        URL: <Copyable text={fn.url} what="the URL" />
                    </span>
                )}
                {fn.message && fn.state !== "ACTIVE" && <span className="gcp-error-text">{fn.message}</span>}
            </div>
            <Tabs label={fn.name} tabs={TABS} value={tab} onChange={(value) => updateGcpView(paneId, { tab: value })} />
            {tab === "metrics" && (
                <div className="gcp-charts">
                    <Sparkline title="Executions" series={executions.data} />
                    {fn.runService && <Sparkline title="Requests" series={requests.data} />}
                </div>
            )}
            {tab === "details" && (
                <div className="gcp-section">
                    <Facts
                        items={[
                            ["Trigger", trigger(fn)],
                            ["Retry on failure", fn.trigger.kind === "event" ? (fn.trigger.retry ? "On" : "Off") : null],
                            ["Runtime", fn.runtime],
                            ["Entry point", fn.entryPoint],
                            ["Memory", fn.memory],
                            ["Timeout", fn.timeoutSeconds ? `${fn.timeoutSeconds}s` : null],
                            ["Max instances", fn.maxInstances],
                            ["Service account", fn.serviceAccount],
                            ["Cloud Run service", fn.runService],
                        ]}
                    />
                    {fn.env.length + fn.secretEnv.length > 0 && (
                        <>
                            <h2 className="gcp-section-title">Environment variables</h2>
                            <p className="gcp-note">Names only. Values stay in Google Cloud.</p>
                            <Table
                                label="Environment variables"
                                rows={[...fn.env.map((name) => ({ name, secret: false })), ...fn.secretEnv.map((name) => ({ name, secret: true }))]}
                                rowKey={(entry) => entry.name}
                                columns={[
                                    { label: "Name", cell: (entry) => <span className="gcp-mono">{entry.name}</span> },
                                    { label: "Source", cell: (entry) => (entry.secret ? "Secret Manager" : "Value") },
                                ]}
                            />
                        </>
                    )}
                </div>
            )}
            {tab === "logs" && (
                <LogStream
                    active={active}
                    target={target}
                    query={{
                        resource: fn.generation === "1st gen" ? "function" : "runService",
                        name: fn.generation === "1st gen" ? fn.name : (fn.runService ?? fn.name),
                    }}
                />
            )}
        </div>
    );
}
