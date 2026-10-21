import { runInTerminal, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { gcpApi, type SqlDetail, type SqlInstance, type Target } from "../api";
import { metricsR, sqlInstanceR, sqlInstancesR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { LogStream } from "./LogStream";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Copyable, Facts, keyOf, Loaded, PageHeader, Sparkline, Status, Table, Tabs, took, useAction, visit } from "./parts";

type Change = "start" | "stop" | "restart" | "backup";

const CHANGES: Record<Change, { label: string; body: (name: string) => string; destructive?: boolean }> = {
    start: { label: "Start", body: (name) => `${name} starts accepting connections, and is billed while it runs.` },
    stop: { label: "Stop", body: (name) => `${name} stops and drops every connection. Its data is kept.`, destructive: true },
    restart: { label: "Restart", body: (name) => `${name} restarts, dropping every connection for a minute or so.`, destructive: true },
    backup: { label: "Back up now", body: (name) => `Cloud SQL takes an on-demand backup of ${name}.` },
};

export function changesFor(state: string): Change[] {
    if (state === "RUNNING") return ["backup", "restart", "stop"];
    if (state === "STOPPED") return ["start"];
    return [];
}

const engine = (version: string) =>
    version.replace("POSTGRES_", "PostgreSQL ").replace("MYSQL_", "MySQL ").replace("SQLSERVER_", "SQL Server ").replace(/_/g, ".");

export function SqlPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    if (view.open)
        return (
            <InstancePage
                paneId={paneId}
                active={active}
                target={target}
                name={view.open}
                onBack={() => updateGcpView(paneId, { open: null, tab: null })}
            />
        );
    return <Instances paneId={paneId} active={active} target={target} />;
}

function Instances({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const instances = useResourceEnabled(active, sqlInstancesR, ...keyOf(target));
    const words = view.filter.trim().toLowerCase();
    return (
        <div className="gcp-page">
            <PageHeader title="Instances">
                <Button onClick={() => visit(consoleUrl("sql/instances", target.project))}>Create instance in Cloud console</Button>
            </PageHeader>
            <Filter paneId={paneId} placeholder="Filter instances" />
            <Loaded handle={instances} label="instances" empty={(rows) => (rows.length === 0 ? "No Cloud SQL instances in this project." : null)}>
                {(rows) => (
                    <Table
                        label="Instances"
                        rows={rows.filter((instance) => !words || instance.name.includes(words))}
                        rowKey={(instance) => instance.name}
                        onOpen={(instance: SqlInstance) => updateGcpView(paneId, { open: instance.name, tab: null })}
                        columns={[
                            { label: "", width: "24px", cell: (instance) => <Status state={instance.state} /> },
                            {
                                label: "Instance ID",
                                width: "minmax(140px, 1.2fr)",
                                cell: (instance) => <span className="gcp-name">{instance.name}</span>,
                            },
                            { label: "Type", cell: (instance) => engine(instance.engine) },
                            { label: "Public IP", cell: (instance) => instance.publicIp ?? "—" },
                            { label: "Private IP", cell: (instance) => instance.privateIp ?? "—" },
                            { label: "Location", cell: (instance) => instance.zone ?? instance.region },
                            { label: "High availability", cell: (instance) => (instance.highAvailability ? "On" : "Off") },
                        ]}
                    />
                )}
            </Loaded>
        </div>
    );
}

const TABS = [
    { value: "overview", label: "Overview" },
    { value: "databases", label: "Databases" },
    { value: "users", label: "Users" },
    { value: "backups", label: "Backups" },
    { value: "logs", label: "Logs" },
] as const;

function InstancePage({
    paneId,
    active,
    target,
    name,
    onBack,
}: {
    paneId: string;
    active: boolean;
    target: Target;
    name: string;
    onBack: () => void;
}) {
    const view = useGcpView(paneId);
    const tab = TABS.find((each) => each.value === view.tab)?.value ?? "overview";
    const instance = useResourceEnabled(active, sqlInstanceR, ...keyOf(target), name);
    const action = useAction();
    const run = (change: Change) =>
        void action.run(
            {
                title: `${CHANGES[change].label}: ${name}?`,
                body: CHANGES[change].body(name),
                confirmLabel: CHANGES[change].label,
                destructive: CHANGES[change].destructive,
            },
            `${CHANGES[change].label} sent to ${name}`,
            () => gcpApi.sqlChange(target, name, change),
        );
    return (
        <div className="gcp-page">
            <PageHeader title={name} back={{ label: "instances", go: onBack }}>
                {instance.data && (
                    <Button
                        primary
                        disabled={instance.data.state !== "RUNNING"}
                        title="Opens gcloud sql connect in a terminal"
                        onClick={() => void runInTerminal(`Connect to ${name}`, instance.data!.connect).catch(swallow("connect"))}>
                        Connect
                    </Button>
                )}
                {instance.data &&
                    changesFor(instance.data.state).map((change) => (
                        <Button key={change} danger={CHANGES[change].destructive} disabled={action.busy} onClick={() => run(change)}>
                            {CHANGES[change].label}
                        </Button>
                    ))}
            </PageHeader>
            <Loaded handle={instance} label={name} empty={() => null}>
                {(detail) => (
                    <>
                        <div className="gcp-summary">
                            <Status state={detail.state} text />
                            <span className="gcp-muted">{engine(detail.engine)}</span>
                            <span className="gcp-muted">{detail.region}</span>
                        </div>
                        <Tabs label={name} tabs={TABS} value={tab} onChange={(value) => updateGcpView(paneId, { tab: value })} />
                        {tab === "overview" && <Overview active={active} target={target} detail={detail} />}
                        {tab === "databases" && (
                            <Table
                                label="Databases"
                                rows={detail.databases}
                                rowKey={(db) => db.name}
                                columns={[
                                    { label: "Name", cell: (db) => db.name },
                                    { label: "Character set", cell: (db) => db.charset ?? "—" },
                                ]}
                            />
                        )}
                        {tab === "users" && (
                            <Table
                                label="Users"
                                rows={detail.users}
                                rowKey={(user) => `${user.name}@${user.host ?? ""}`}
                                columns={[
                                    { label: "User name", cell: (user) => user.name },
                                    { label: "Host", cell: (user) => user.host || "Any" },
                                    {
                                        label: "Authentication",
                                        cell: (user) => (user.type ?? "BUILT_IN").replace("CLOUD_IAM_", "IAM ").replace(/_/g, " ").toLowerCase(),
                                    },
                                ]}
                            />
                        )}
                        {tab === "backups" && (
                            <Table
                                label="Backups"
                                rows={detail.recentBackups}
                                rowKey={(run) => run.id}
                                columns={[
                                    {
                                        label: "",
                                        width: "24px",
                                        cell: (run) => <Status state={run.status === "SUCCESSFUL" ? "SUCCESS" : run.status} />,
                                    },
                                    { label: "Started", cell: (run) => ago(run.startedAt) },
                                    { label: "Took", cell: (run) => took(run.startedAt, run.endedAt) },
                                    { label: "Type", cell: (run) => (run.type === "ON_DEMAND" ? "On demand" : "Automated") },
                                ]}
                            />
                        )}
                        {tab === "logs" && <LogStream active={active} target={target} query={{ resource: "sql", name }} />}
                    </>
                )}
            </Loaded>
        </div>
    );
}

function Overview({ active, target, detail }: { active: boolean; target: Target; detail: SqlDetail }) {
    const key = keyOf(target);
    const cpu = useResourceEnabled(active, metricsR, ...key, "sqlCpu", detail.name, "6h");
    const memory = useResourceEnabled(active, metricsR, ...key, "sqlMemory", detail.name, "6h");
    const connections = useResourceEnabled(active, metricsR, ...key, "sqlConnections", detail.name, "6h");
    return (
        <div className="gcp-section">
            <div className="gcp-charts">
                <Sparkline title="CPU utilisation" series={cpu.data} />
                <Sparkline title="Memory utilisation" series={memory.data} />
                <Sparkline title="Connections" series={connections.data} />
            </div>
            <Facts
                items={[
                    ["Connection name", <Copyable key="c" text={detail.connectionName} what="the connection name" />],
                    ["Public IP", detail.publicIp ?? "None"],
                    ["Private IP", detail.privateIp ?? "None"],
                    ["Machine type", detail.tier],
                    ["Storage", detail.diskGb ? `${detail.diskGb} GB` : null],
                    ["High availability", detail.highAvailability ? "Regional" : "Zonal"],
                    ["Automated backups", detail.backups ? `On, starting ${detail.backupWindow ?? "daily"} UTC` : "Off"],
                    ["Created", ago(detail.createdAt)],
                ]}
            />
        </div>
    );
}
