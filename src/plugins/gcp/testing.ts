import { vi } from "vitest";
import type { GcpStatus } from "./api";

/** Every backend call the console makes, each a mock a test can answer. */
export function mockApi() {
    return {
        status: vi.fn(),
        projects: vi.fn(),
        agentPicks: vi.fn(),
        pickForAgents: vi.fn(),
        runServices: vi.fn(),
        runService: vi.fn(),
        runSetTraffic: vi.fn(),
        runSplitTraffic: vi.fn(),
        runDeploy: vi.fn(),
        runJobs: vi.fn(),
        runJob: vi.fn(),
        runJobExecute: vi.fn(),
        functions: vi.fn(),
        instances: vi.fn(),
        instance: vi.fn(),
        instancePower: vi.fn(),
        instanceSerial: vi.fn(),
        snapshots: vi.fn(),
        snapshotDisk: vi.fn(),
        clusters: vi.fn(),
        workloads: vi.fn(),
        pods: vi.fn(),
        kubeServices: vi.fn(),
        podLogs: vi.fn(),
        nodes: vi.fn(),
        podEvents: vi.fn(),
        deploymentRestart: vi.fn(),
        deploymentScale: vi.fn(),
        nodePoolResize: vi.fn(),
        buckets: vi.fn(),
        objects: vi.fn(),
        objectPreview: vi.fn(),
        objectDownload: vi.fn(),
        objectUpload: vi.fn(),
        sqlInstances: vi.fn(),
        sqlInstance: vi.fn(),
        sqlChange: vi.fn(),
        pubsub: vi.fn(),
        publish: vi.fn(),
        peek: vi.fn(),
        builds: vi.fn(),
        build: vi.fn(),
        buildChange: vi.fn(),
        triggers: vi.fn(),
        triggerRun: vi.fn(),
        secrets: vi.fn(),
        secret: vi.fn(),
        secretVersion: vi.fn(),
        logs: vi.fn(),
        tailLogs: vi.fn(),
        metrics: vi.fn(),
        errorGroups: vi.fn(),
        errorEvents: vi.fn(),
        errorStatus: vi.fn(),
        repositories: vi.fn(),
        repository: vi.fn(),
        schedulerJobs: vi.fn(),
        schedulerChange: vi.fn(),
        iamMembers: vi.fn(),
        serviceAccounts: vi.fn(),
        billing: vi.fn(),
        enableService: vi.fn(),
        network: vi.fn(),
        firewallToggle: vi.fn(),
        dnsZones: vi.fn(),
        dnsRecords: vi.fn(),
        alerting: vi.fn(),
        alertPolicyToggle: vi.fn(),
        datasets: vi.fn(),
        tables: vi.fn(),
        table: vi.fn(),
        query: vi.fn(),
        signIn: vi.fn(),
        signInApplicationDefault: vi.fn(),
    };
}

export const signedIn: GcpStatus = {
    installed: true,
    accounts: [{ account: "ankit@example.com", active: true, serviceAccount: false }],
    configurations: [
        { name: "default", active: true, account: "ankit@example.com", project: "acme-prod", region: "us-central1", zone: "us-central1-a" },
    ],
    applicationDefault: false,
    message: null,
};

/** Answers every list with nothing, so a test only fills in what it reads. */
export function answerEmpty(api: ReturnType<typeof mockApi>) {
    api.status.mockResolvedValue(signedIn);
    api.projects.mockResolvedValue([{ id: "acme-prod", name: "Acme Prod", number: "1", createdAt: "" }]);
    for (const list of [
        "runServices",
        "runJobs",
        "functions",
        "instances",
        "clusters",
        "buckets",
        "sqlInstances",
        "builds",
        "triggers",
        "secrets",
        "logs",
        "errorGroups",
        "errorEvents",
        "repositories",
        "schedulerJobs",
        "iamMembers",
        "serviceAccounts",
        "datasets",
        "tables",
        "nodes",
        "podEvents",
        "snapshots",
        "dnsZones",
        "dnsRecords",
    ] as const)
        api[list].mockResolvedValue([]);
    api.pubsub.mockResolvedValue({ topics: [], subscriptions: [] });
    api.network.mockResolvedValue({ networks: [], subnets: [], firewalls: [] });
    api.alerting.mockResolvedValue({ policies: [], uptimeChecks: [], channels: [] });
    api.billing.mockResolvedValue({ enabled: true, account: "0A1B2C-3D4E5F-6A7B8C", accountName: null, accountOpen: null, budgets: null });
    api.metrics.mockResolvedValue({ unit: "", step: 60, points: [] });
    api.agentPicks.mockResolvedValue({ project: null, default: null, effective: null });
    api.tailLogs.mockReturnValue({ stop: vi.fn() });
}
