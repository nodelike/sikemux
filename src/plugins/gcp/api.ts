import { createPluginBackend, type PluginStream } from "../../plugin-api/backend";
import { GCP_PLUGIN } from "./kinds";

const backend = createPluginBackend(GCP_PLUGIN.id);

export interface GcpAccount {
    account: string;
    active: boolean;
    serviceAccount: boolean;
}

export interface GcpConfiguration {
    name: string;
    active: boolean;
    account: string | null;
    project: string | null;
    region: string | null;
    zone: string | null;
}

export interface GcpStatus {
    installed: boolean;
    accounts: GcpAccount[];
    configurations: GcpConfiguration[];
    applicationDefault: boolean;
    message: string | null;
}

/** Which account and project a call runs as. */
export interface Target {
    account: string | null;
    project: string;
}

export interface GcpProject {
    id: string;
    name: string;
    number: string;
    createdAt: string;
}

export interface TrafficEntry {
    revision: string | null;
    latest: boolean;
    percent: number;
    tag: string | null;
    url: string | null;
}

export interface RunService {
    name: string;
    location: string;
    url: string | null;
    state: "READY" | "FAILED" | "DEPLOYING" | "UNKNOWN";
    message: string | null;
    latestRevision: string;
    image: string | null;
    cpu: string | null;
    memory: string | null;
    minInstances: number | null;
    maxInstances: number | null;
    concurrency: number | null;
    serviceAccount: string | null;
    ingress: string | null;
    traffic: TrafficEntry[];
    updatedAt: string | null;
    updatedBy: string | null;
}

export interface RunRevision {
    name: string;
    state: string;
    image: string | null;
    createdAt: string | null;
    percent: number;
    tags: string[];
}

export interface RunServiceDetail extends RunService {
    revisions: RunRevision[];
    env: { name: string; secret: string | null }[];
}

export interface RunJob {
    name: string;
    location: string;
    state: string;
    image: string | null;
    tasks: number | null;
    executions: number | null;
    latestExecution: string;
    latestExecutionAt: string | null;
    updatedAt: string | null;
}

export interface RunExecution {
    name: string;
    state: "RUNNING" | "FAILED" | "PENDING" | "SUCCEEDED" | "CANCELLED";
    tasks: number | null;
    succeeded: number;
    failed: number;
    running: number;
    startedAt: string | null;
    completedAt: string | null;
    createdAt: string | null;
}

export interface RunJobDetail extends RunJob {
    recent: RunExecution[];
}

export interface GcpFunction {
    name: string;
    location: string;
    state: string;
    generation: string;
    runtime: string | null;
    entryPoint: string | null;
    url: string | null;
    memory: string | null;
    timeoutSeconds: number | null;
    maxInstances: number | null;
    serviceAccount: string | null;
    runService: string | null;
    trigger: { kind: "http" } | { kind: "event"; event: string; topic: string | null; retry: boolean };
    env: string[];
    secretEnv: string[];
    updatedAt: string | null;
    message: string | null;
}

export interface VmInstance {
    id: string;
    name: string;
    zone: string;
    machineType: string;
    status: string;
    internalIps: string[];
    externalIps: string[];
    disks: { name: string; sizeGb: number | null; boot: boolean; mode: string | null }[];
    spot: boolean;
    serviceAccount: string | null;
    createdAt: string | null;
    startedAt: string | null;
}

export interface VmDetail extends VmInstance {
    ssh: string[];
}

export interface Snapshot {
    name: string;
    disk: string;
    status: string;
    sizeGb: number | null;
    storageBytes: number | null;
    locations: string[] | null;
    createdAt: string | null;
}

export interface SerialOutput {
    contents: string;
    start: string | number | null;
    next: string | number | null;
    trimmed: boolean;
}

export interface NodePool {
    name: string;
    status: string;
    version: string | null;
    machineType: string | null;
    diskSizeGb: number | null;
    spot: boolean;
    nodes: number | null;
    autoscaling: boolean;
    minNodes: number | null;
    maxNodes: number | null;
}

export interface GkeCluster {
    name: string;
    location: string;
    status: string;
    message: string | null;
    version: string | null;
    nodeVersion: string | null;
    nodes: number | null;
    autopilot: boolean;
    channel: string | null;
    network: string;
    private: boolean;
    nodePools: NodePool[];
    createdAt: string | null;
}

export interface Workload {
    kind: "Deployment" | "StatefulSet" | "DaemonSet";
    name: string;
    namespace: string;
    desired: number;
    ready: number;
    containers: { name: string; image: string }[];
    selector: string;
    createdAt: string | null;
}

export interface Pod {
    name: string;
    namespace: string;
    phase: string;
    reason: string | null;
    ready: string;
    restarts: number;
    node: string | null;
    ip: string | null;
    containers: { name: string; image: string }[];
    createdAt: string | null;
}

export interface KubeService {
    name: string;
    namespace: string;
    type: string;
    clusterIp: string | null;
    externalIps: string[];
    ports: string[];
}

export interface KubeNode {
    name: string;
    ready: boolean;
    pressure: string[];
    unschedulable: boolean;
    pool: string | null;
    machineType: string | null;
    zone: string | null;
    spot: boolean;
    cpu: string | null;
    memory: string | null;
    pods: string | null;
    version: string | null;
    createdAt: string | null;
}

export interface KubeEvent {
    type: string;
    reason: string;
    message: string;
    count: number;
    lastAt: string | null;
    source: string | null;
}

export interface PodLogLine {
    at: string | null;
    text: string;
}

export interface Bucket {
    name: string;
    location: string;
    locationType: string | null;
    storageClass: string;
    publicAccessPrevention: string | null;
    uniformAccess: boolean;
    versioning: boolean;
    createdAt: string | null;
}

export interface StorageObject {
    name: string;
    size: number | null;
    contentType: string | null;
    storageClass: string | null;
    updatedAt: string | null;
}

export interface Folder {
    bucket: string;
    prefix: string;
    folders: string[];
    objects: StorageObject[];
    more: boolean;
}

export interface ObjectPreview extends StorageObject {
    bucket: string;
    preview: string | null;
    trimmed: boolean;
    uri: string;
}

export interface SqlInstance {
    name: string;
    engine: string;
    region: string;
    zone: string | null;
    state: string;
    tier: string | null;
    diskGb: number | null;
    highAvailability: boolean;
    backups: boolean;
    backupWindow: string | null;
    publicIp: string | null;
    privateIp: string | null;
    connectionName: string;
    createdAt: string | null;
}

export interface SqlDetail extends SqlInstance {
    databases: { name: string; charset: string | null }[];
    users: { name: string; host: string | null; type: string | null }[];
    recentBackups: { id: string; status: string; type: string; startedAt: string | null; endedAt: string | null }[];
    connect: string[];
}

export interface Topic {
    name: string;
    retention: string | null;
}

export interface Subscription {
    name: string;
    topic: string;
    kind: "pull" | "push" | "bigquery" | "storage";
    pushEndpoint: string | null;
    ackDeadlineSeconds: number | null;
    retention: string | null;
    deadLetterTopic: string | null;
    maxDeliveryAttempts: number | null;
    filter: string | null;
    ordering: boolean;
    state: string | null;
}

export interface PubSub {
    topics: Topic[];
    subscriptions: Subscription[];
}

export interface PeekedMessage {
    id: string;
    publishedAt: string;
    data: { text: string } | { base64: string };
    attributes: Record<string, string> | null;
    deliveryAttempt: number | null;
}

export interface BuildStep {
    id: string | null;
    image: string;
    status: string | null;
    startedAt: string | null;
    endedAt: string | null;
}

export interface Build {
    id: string;
    status: string;
    detail: string | null;
    failure: string | null;
    trigger: string | null;
    triggerId: string | null;
    repo: string | null;
    branch: string | null;
    commit: string | null;
    steps: BuildStep[];
    images: string[] | null;
    createdAt: string | null;
    startedAt: string | null;
    finishedAt: string | null;
    logUrl: string | null;
    region: string | null;
}

export interface BuildDetail extends Build {
    log: string[];
    logSkipped: number;
}

export interface Trigger {
    id: string;
    name: string;
    description: string | null;
    repo: string | null;
    branch: string | null;
    pullRequests: boolean;
    file: string | null;
    disabled: boolean;
}

export interface Secret {
    name: string;
    replication: "automatic" | string[];
    createdAt: string | null;
    expiresAt: string | null;
    nextRotation: string | null;
    rotationPeriod: string | null;
}

export interface SecretDetail extends Secret {
    versions: { version: string; state: string; createdAt: string | null; destroyedAt: string | null }[];
}

export type LogResource = "runService" | "runJob" | "instance" | "gkeCluster" | "function" | "build" | "sql";

export interface LogQuery {
    resource?: LogResource | null;
    name?: string | null;
    severity?: string | null;
    since?: string | null;
    text?: string | null;
    filter?: string | null;
    limit?: number;
}

export interface LogEntry {
    id: string;
    at: string;
    severity: string;
    text: string;
    log: string;
    resource: string | null;
    labels: Record<string, string> | null;
    http: { method: string | null; url: string | null; status: number | null; latency: string | null } | null;
    json: unknown;
    trace: string | null;
}

export type ChartKind =
    | "runRequests"
    | "runErrors"
    | "runLatency"
    | "runInstances"
    | "instanceCpu"
    | "sqlCpu"
    | "sqlMemory"
    | "sqlConnections"
    | "subscriptionBacklog"
    | "subscriptionOldest"
    | "functionExecutions";

export interface Series {
    unit: string;
    step: number;
    points: { at: string; value: number }[];
}

export interface ErrorSample {
    message: string;
    at: string | null;
    service: string | null;
    version: string | null;
    file: string | null;
    line: number | null;
    function: string | null;
    request: { method: string | null; url: string | null; status: number | null } | null;
    user: string | null;
}

export type ErrorStatus = "OPEN" | "ACKNOWLEDGED" | "RESOLVED" | "MUTED";

export interface ErrorGroup {
    id: string;
    title: string;
    status: ErrorStatus;
    count: number | null;
    users: number | null;
    firstSeen: string | null;
    lastSeen: string | null;
    services: string[];
    timeline: number[];
    issue: string | null;
    sample: ErrorSample;
}

export interface Repository {
    name: string;
    location: string;
    format: string;
    mode: string | null;
    description: string | null;
    sizeBytes: number | null;
    updatedAt: string | null;
}

export interface DockerImage {
    digest: string;
    image: string;
    uri: string;
    tags: string[];
    sizeBytes: number | null;
    uploadedAt: string | null;
    builtAt: string | null;
}

export type RepositoryContents =
    { format: "DOCKER"; images: DockerImage[] } | { format: string; packages: { name: string; updatedAt: string | null }[] };

export interface SchedulerJob {
    name: string;
    location: string;
    description: string | null;
    schedule: string | null;
    timeZone: string | null;
    state: string;
    target: { kind: "http"; method: string; uri: string } | { kind: "pubsub"; topic: string } | { kind: "appEngine"; uri: string } | null;
    lastRunAt: string | null;
    nextRunAt: string | null;
    lastRunOk: boolean | null;
    lastRunError: string | null;
}

export interface IamMember {
    member: string;
    kind: string;
    roles: string[];
}

export interface ServiceAccount {
    email: string;
    name: string | null;
    description: string | null;
    disabled: boolean;
}

export interface BqField {
    name: string;
    type: string;
    mode: string;
    description: string | null;
    fields: BqField[];
}

export interface BqTable {
    id: string;
    type: string;
    description: string | null;
    rows: number | null;
    bytes: number | null;
    location: string | null;
    modifiedAt: number | null;
    partitionedBy: string | null;
    clusteredBy: string[] | null;
    view: string | null;
    schema: BqField[];
    preview: Record<string, unknown>[] | null;
}

export type BqResult =
    | { dryRun: true; bytes: number; costUsd: number; schema: BqField[] }
    | {
          dryRun: false;
          schema: BqField[];
          rows: Record<string, unknown>[];
          totalRows: number | null;
          bytes: number;
          costUsd: number;
          cached: boolean;
          job: string;
      };

export interface Billing {
    enabled: boolean;
    account: string | null;
    accountName: string | null;
    accountOpen: boolean | null;
    budgets:
        { name: string; amount: number | null; currency: string | null; lastPeriod: boolean; thresholds: number[]; projects: number | null }[] | null;
}

export interface Network {
    name: string;
    mode: "auto" | "custom";
    routing: string | null;
    mtu: number | null;
    subnets: number;
    createdAt: string | null;
}

export interface Subnet {
    name: string;
    region: string;
    network: string;
    range: string;
    secondary: string[];
    privateGoogleAccess: boolean;
    purpose: string | null;
}

export interface FirewallRule {
    name: string;
    network: string;
    direction: "INGRESS" | "EGRESS";
    priority: number;
    action: "allow" | "deny";
    protocols: string[];
    sources: string[];
    destinations: string[];
    targets: string[];
    disabled: boolean;
    logging: boolean;
    description: string | null;
}

export interface NetworkOverview {
    networks: Network[];
    subnets: Subnet[];
    firewalls: FirewallRule[];
}

export interface DnsZone {
    name: string;
    dnsName: string;
    visibility: string;
    dnssec: boolean;
    nameServers: string[] | null;
    description: string | null;
}

export interface DnsRecord {
    name: string;
    type: string;
    ttl: number | null;
    data: string[];
}

export interface Alerting {
    policies: {
        id: string;
        name: string;
        enabled: boolean;
        conditions: { name: string; summary: string }[];
        combiner: string | null;
        channels: number;
        documentation: string | null;
    }[];
    uptimeChecks: { id: string; name: string; target: string; period: string | null; timeout: string | null; regions: string[] | null }[];
    channels: { name: string; type: string; enabled: boolean; verified: boolean }[];
}

export interface AgentPick {
    account: string | null;
    project: string | null;
}

export interface AgentPicks {
    project: AgentPick | null;
    default: AgentPick | null;
    effective: { pick: AgentPick; source: "environment" | "project" | "default" } | null;
}

export interface Operation {
    operation: string | null;
    status?: string | null;
}

const t = (target: Target) => ({ account: target.account, project: target.project });

export const gcpApi = {
    status: () => backend.call<GcpStatus>("status"),
    projects: (account: string | null) => backend.call<GcpProject[]>("projects", { account }),
    agentPicks: (folder: string | null) => backend.call<AgentPicks>("agentPicks", { folder }),
    pickForAgents: (folder: string | null, pick: AgentPick) => backend.call<AgentPicks>("pickForAgents", { folder, ...pick }),

    runServices: (target: Target) => backend.call<RunService[]>("runServices", t(target)),
    runService: (target: Target, location: string, service: string) =>
        backend.call<RunServiceDetail>("runService", { ...t(target), location, service }),
    runSetTraffic: (target: Target, location: string, service: string, revision: string | null) =>
        backend.call<Operation>("runSetTraffic", { ...t(target), location, service, revision }),
    runSplitTraffic: (target: Target, location: string, service: string, split: { revision: string | null; percent: number }[]) =>
        backend.call<Operation>("runSetTraffic", { ...t(target), location, service, split }),
    runDeploy: (target: Target, location: string, service: string, change: { image?: string; minInstances?: number; maxInstances?: number }) =>
        backend.call<Operation>("runDeploy", { ...t(target), location, service, ...change }),
    runJobs: (target: Target) => backend.call<RunJob[]>("runJobs", t(target)),
    runJob: (target: Target, location: string, job: string) => backend.call<RunJobDetail>("runJob", { ...t(target), location, job }),
    runJobExecute: (target: Target, location: string, job: string) => backend.call<Operation>("runJobExecute", { ...t(target), location, job }),

    functions: (target: Target) => backend.call<GcpFunction[]>("functions", t(target)),

    instances: (target: Target) => backend.call<VmInstance[]>("instances", t(target)),
    instance: (target: Target, zone: string, instance: string) => backend.call<VmDetail>("instance", { ...t(target), zone, instance }),
    instancePower: (target: Target, zone: string, instance: string, action: "start" | "stop" | "reset" | "suspend" | "resume") =>
        backend.call<Operation>("instancePower", { ...t(target), zone, instance, action }),
    instanceSerial: (target: Target, zone: string, instance: string) =>
        backend.call<SerialOutput>("instanceSerial", { ...t(target), zone, instance }),

    snapshots: (target: Target) => backend.call<Snapshot[]>("snapshots", t(target)),
    snapshotDisk: (target: Target, zone: string, disk: string) =>
        backend.call<Operation & { snapshot: string }>("snapshotDisk", { ...t(target), zone, disk }),
    clusters: (target: Target) => backend.call<GkeCluster[]>("clusters", t(target)),
    workloads: (target: Target, location: string, cluster: string) => backend.call<Workload[]>("workloads", { ...t(target), location, cluster }),
    pods: (target: Target, location: string, cluster: string, selector: string | null) =>
        backend.call<Pod[]>("pods", { ...t(target), location, cluster, selector }),
    kubeServices: (target: Target, location: string, cluster: string) =>
        backend.call<KubeService[]>("kubeServices", { ...t(target), location, cluster }),
    nodes: (target: Target, location: string, cluster: string) => backend.call<KubeNode[]>("nodes", { ...t(target), location, cluster }),
    podEvents: (target: Target, location: string, cluster: string, namespace: string, pod: string) =>
        backend.call<KubeEvent[]>("podEvents", { ...t(target), location, cluster, namespace, pod }),
    podLogs: (target: Target, location: string, cluster: string, namespace: string, pod: string, previous: boolean) =>
        backend.call<{ lines: PodLogLine[] }>("podLogs", { ...t(target), location, cluster, namespace, pod, previous, tail: 1000 }),
    deploymentRestart: (target: Target, location: string, cluster: string, namespace: string, deployment: string) =>
        backend.call<Workload>("deploymentRestart", { ...t(target), location, cluster, namespace, deployment }),
    deploymentScale: (target: Target, location: string, cluster: string, namespace: string, deployment: string, replicas: number) =>
        backend.call<{ replicas: number }>("deploymentScale", { ...t(target), location, cluster, namespace, deployment, replicas }),
    nodePoolResize: (target: Target, location: string, cluster: string, nodePool: string, nodes: number) =>
        backend.call<Operation>("nodePoolResize", { ...t(target), location, cluster, nodePool, nodes }),

    buckets: (target: Target) => backend.call<Bucket[]>("buckets", t(target)),
    objects: (target: Target, bucket: string, prefix: string) => backend.call<Folder>("objects", { ...t(target), bucket, prefix }),
    objectPreview: (target: Target, bucket: string, object: string) => backend.call<ObjectPreview>("objectPreview", { ...t(target), bucket, object }),
    objectUpload: (target: Target, bucket: string, prefix: string, path: string) =>
        backend.call<StorageObject>("objectUpload", { ...t(target), bucket, prefix, path }),
    objectDownload: (target: Target, bucket: string, object: string) =>
        backend.call<{ path: string; size: number }>("objectDownload", { ...t(target), bucket, object }),

    sqlInstances: (target: Target) => backend.call<SqlInstance[]>("sqlInstances", t(target)),
    sqlInstance: (target: Target, instance: string) => backend.call<SqlDetail>("sqlInstance", { ...t(target), instance }),
    sqlChange: (target: Target, instance: string, action: "start" | "stop" | "restart" | "backup") =>
        backend.call<Operation>("sqlChange", { ...t(target), instance, action }),

    pubsub: (target: Target) => backend.call<PubSub>("topics", t(target)),
    publish: (target: Target, topic: string, data: string, attributes: Record<string, string>) =>
        backend.call<{ messageId: string }>("publish", { ...t(target), topic, data, attributes }),
    peek: (target: Target, subscription: string) => backend.call<PeekedMessage[]>("peek", { ...t(target), subscription, limit: 20 }),

    builds: (target: Target) => backend.call<Build[]>("builds", { ...t(target), limit: 50 }),
    build: (target: Target, id: string, region: string | null) => backend.call<BuildDetail>("build", { ...t(target), id, region }),
    buildChange: (target: Target, id: string, region: string | null, action: "retry" | "cancel") =>
        backend.call<Build>("buildChange", { ...t(target), id, region, action }),
    triggers: (target: Target) => backend.call<Trigger[]>("triggers", t(target)),
    triggerRun: (target: Target, trigger: string, branch: string | null) => backend.call<Build>("triggerRun", { ...t(target), trigger, branch }),

    secrets: (target: Target) => backend.call<Secret[]>("secrets", t(target)),
    secret: (target: Target, secret: string) => backend.call<SecretDetail>("secret", { ...t(target), secret }),
    secretVersion: (target: Target, secret: string, version: string, enabled: boolean) =>
        backend.call<unknown>("secretVersion", { ...t(target), secret, version, enabled }),

    logs: (target: Target, query: LogQuery) => backend.call<LogEntry[]>("logs", { ...t(target), ...query }),
    tailLogs: (
        target: Target,
        query: LogQuery,
        handlers: { onEntries: (entries: LogEntry[]) => void; onError: (message: string) => void },
    ): PluginStream =>
        backend.stream<LogEntry[]>(
            "tailLogs",
            { ...t(target), ...query },
            { onItem: handlers.onEntries, onError: (error) => handlers.onError(error.message) },
        ),

    errorGroups: (target: Target, since: string) => backend.call<ErrorGroup[]>("errorGroups", { ...t(target), since }),
    errorEvents: (target: Target, group: string) => backend.call<ErrorSample[]>("errorEvents", { ...t(target), group }),
    errorStatus: (target: Target, group: string, status: ErrorStatus) =>
        backend.call<{ id: string; status: ErrorStatus }>("errorStatus", { ...t(target), group, status }),

    repositories: (target: Target) => backend.call<Repository[]>("repositories", t(target)),
    repository: (target: Target, location: string, repository: string) =>
        backend.call<RepositoryContents>("repository", { ...t(target), location, repository }),

    schedulerJobs: (target: Target) => backend.call<SchedulerJob[]>("schedulerJobs", t(target)),
    schedulerChange: (target: Target, location: string, job: string, action: "run" | "pause" | "resume") =>
        backend.call<SchedulerJob>("schedulerChange", { ...t(target), location, job, action }),

    iamMembers: (target: Target) => backend.call<IamMember[]>("iamMembers", t(target)),
    serviceAccounts: (target: Target) => backend.call<ServiceAccount[]>("serviceAccounts", t(target)),

    network: (target: Target) => backend.call<NetworkOverview>("network", t(target)),
    firewallToggle: (target: Target, rule: string, disabled: boolean) => backend.call<Operation>("firewallToggle", { ...t(target), rule, disabled }),
    dnsZones: (target: Target) => backend.call<DnsZone[]>("dnsZones", t(target)),
    dnsRecords: (target: Target, zone: string) => backend.call<DnsRecord[]>("dnsRecords", { ...t(target), zone }),
    alerting: (target: Target) => backend.call<Alerting>("alerting", t(target)),
    alertPolicyToggle: (target: Target, policy: string, enabled: boolean) =>
        backend.call<unknown>("alertPolicyToggle", { ...t(target), policy, enabled }),
    billing: (target: Target) => backend.call<Billing>("billing", t(target)),
    enableService: (target: Target, service: string) =>
        backend.call<{ service: string; enabled: boolean }>("enableService", { ...t(target), service }),
    datasets: (target: Target) => backend.call<{ id: string; location: string | null; name: string | null }[]>("datasets", t(target)),
    tables: (target: Target, dataset: string) =>
        backend.call<{ id: string; type: string; partitioned: boolean }[]>("tables", { ...t(target), dataset }),
    table: (target: Target, dataset: string, table: string) => backend.call<BqTable>("table", { ...t(target), dataset, table }),
    query: (target: Target, sql: string, dryRun: boolean) => backend.call<BqResult>("query", { ...t(target), sql, dryRun }),

    metrics: (target: Target, chart: ChartKind, name: string, since: string) => backend.call<Series>("metrics", { ...t(target), chart, name, since }),

    signIn: (handlers: { onDone: () => void; onError: (message: string) => void }): PluginStream =>
        backend.stream<GcpStatus>("signIn", {}, { onItem: handlers.onDone, onError: (error) => handlers.onError(error.message) }),
    signInApplicationDefault: (handlers: { onDone: () => void; onError: (message: string) => void }): PluginStream =>
        backend.stream<GcpStatus>("signInApplicationDefault", {}, { onItem: handlers.onDone, onError: (error) => handlers.onError(error.message) }),
};
