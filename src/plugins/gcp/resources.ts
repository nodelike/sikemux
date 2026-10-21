import { invalidate } from "../../plugin-api/resources";
import { pluginResources } from "../../plugin-api/service";
import { gcpApi, type ChartKind, type Target } from "./api";
import { GCP_PLUGIN } from "./kinds";

export const gcp = pluginResources(GCP_PLUGIN);

export const gcpStatusR = gcp.resource("status", () => gcpApi.status(), 60_000);

/** Resources are keyed by plain strings; an empty account means gcloud's active one. */
const target = (account: string, project: string): Target => ({ account: account || null, project });

export const gcpProjectsR = gcp.resource("projects", (account: string) => gcpApi.projects(account || null), 5 * 60_000);

export const runServicesR = gcp.resource("run.services", (account: string, project: string) => gcpApi.runServices(target(account, project)), 30_000);
export const runServiceR = gcp.resource(
    "run.service",
    (account: string, project: string, location: string, service: string) => gcpApi.runService(target(account, project), location, service),
    15_000,
);
export const runJobsR = gcp.resource("run.jobs", (account: string, project: string) => gcpApi.runJobs(target(account, project)), 30_000);
export const runJobR = gcp.resource(
    "run.job",
    (account: string, project: string, location: string, job: string) => gcpApi.runJob(target(account, project), location, job),
    15_000,
);
export const functionsR = gcp.resource("functions", (account: string, project: string) => gcpApi.functions(target(account, project)), 30_000);
export const instancesR = gcp.resource("instances", (account: string, project: string) => gcpApi.instances(target(account, project)), 20_000);
export const instanceR = gcp.resource(
    "instance",
    (account: string, project: string, zone: string, instance: string) => gcpApi.instance(target(account, project), zone, instance),
    15_000,
);
export const serialR = gcp.resource(
    "serial",
    (account: string, project: string, zone: string, instance: string) => gcpApi.instanceSerial(target(account, project), zone, instance),
    10_000,
);
export const clustersR = gcp.resource("clusters", (account: string, project: string) => gcpApi.clusters(target(account, project)), 30_000);
export const workloadsR = gcp.resource(
    "kube.workloads",
    (account: string, project: string, location: string, cluster: string) => gcpApi.workloads(target(account, project), location, cluster),
    15_000,
);
export const podsR = gcp.resource(
    "kube.pods",
    (account: string, project: string, location: string, cluster: string, selector: string) =>
        gcpApi.pods(target(account, project), location, cluster, selector || null),
    10_000,
);
export const kubeServicesR = gcp.resource(
    "kube.services",
    (account: string, project: string, location: string, cluster: string) => gcpApi.kubeServices(target(account, project), location, cluster),
    30_000,
);
export const bucketsR = gcp.resource("buckets", (account: string, project: string) => gcpApi.buckets(target(account, project)), 60_000);
export const folderR = gcp.resource(
    "objects",
    (account: string, project: string, bucket: string, prefix: string) => gcpApi.objects(target(account, project), bucket, prefix),
    30_000,
);
export const sqlInstancesR = gcp.resource("sql", (account: string, project: string) => gcpApi.sqlInstances(target(account, project)), 30_000);
export const sqlInstanceR = gcp.resource(
    "sql.instance",
    (account: string, project: string, instance: string) => gcpApi.sqlInstance(target(account, project), instance),
    20_000,
);
export const pubsubR = gcp.resource("pubsub", (account: string, project: string) => gcpApi.pubsub(target(account, project)), 30_000);
export const buildsR = gcp.resource("builds", (account: string, project: string) => gcpApi.builds(target(account, project)), 15_000);
export const buildR = gcp.resource(
    "build",
    (account: string, project: string, id: string, region: string) => gcpApi.build(target(account, project), id, region || null),
    10_000,
);
export const triggersR = gcp.resource("triggers", (account: string, project: string) => gcpApi.triggers(target(account, project)), 60_000);
export const secretsR = gcp.resource("secrets", (account: string, project: string) => gcpApi.secrets(target(account, project)), 60_000);
export const secretR = gcp.resource(
    "secret",
    (account: string, project: string, secret: string) => gcpApi.secret(target(account, project), secret),
    30_000,
);
export const metricsR = gcp.resource(
    "metrics",
    (account: string, project: string, chart: ChartKind, name: string, since: string) => gcpApi.metrics(target(account, project), chart, name, since),
    60_000,
);

export const recentErrorsR = gcp.resource(
    "logs.errors",
    (account: string, project: string) => gcpApi.logs(target(account, project), { severity: "ERROR", since: "1h", limit: 8 }),
    60_000,
);

export const agentPicksR = gcp.resource("agentPicks", (folder: string | null) => gcpApi.agentPicks(folder), 60_000);

export const errorGroupsR = gcp.resource(
    "errors",
    (account: string, project: string, since: string) => gcpApi.errorGroups(target(account, project), since),
    60_000,
);
export const errorEventsR = gcp.resource(
    "errors.events",
    (account: string, project: string, group: string) => gcpApi.errorEvents(target(account, project), group),
    60_000,
);
export const repositoriesR = gcp.resource(
    "repositories",
    (account: string, project: string) => gcpApi.repositories(target(account, project)),
    60_000,
);
export const repositoryR = gcp.resource(
    "repository",
    (account: string, project: string, location: string, repository: string) => gcpApi.repository(target(account, project), location, repository),
    60_000,
);
export const schedulerJobsR = gcp.resource("scheduler", (account: string, project: string) => gcpApi.schedulerJobs(target(account, project)), 30_000);
export const iamMembersR = gcp.resource("iam", (account: string, project: string) => gcpApi.iamMembers(target(account, project)), 5 * 60_000);
export const serviceAccountsR = gcp.resource(
    "iam.accounts",
    (account: string, project: string) => gcpApi.serviceAccounts(target(account, project)),
    5 * 60_000,
);
export const datasetsR = gcp.resource("bq.datasets", (account: string, project: string) => gcpApi.datasets(target(account, project)), 5 * 60_000);
export const tablesR = gcp.resource(
    "bq.tables",
    (account: string, project: string, dataset: string) => gcpApi.tables(target(account, project), dataset),
    5 * 60_000,
);
export const tableR = gcp.resource(
    "bq.table",
    (account: string, project: string, dataset: string, table: string) => gcpApi.table(target(account, project), dataset, table),
    5 * 60_000,
);

export const nodesR = gcp.resource(
    "kube.nodes",
    (account: string, project: string, location: string, cluster: string) => gcpApi.nodes(target(account, project), location, cluster),
    30_000,
);
export const podEventsR = gcp.resource(
    "kube.events",
    (account: string, project: string, location: string, cluster: string, namespace: string, pod: string) =>
        gcpApi.podEvents(target(account, project), location, cluster, namespace, pod),
    10_000,
);

export const billingR = gcp.resource("billing", (account: string, project: string) => gcpApi.billing(target(account, project)), 10 * 60_000);

export const snapshotsR = gcp.resource("snapshots", (account: string, project: string) => gcpApi.snapshots(target(account, project)), 60_000);

export const networkR = gcp.resource("network", (account: string, project: string) => gcpApi.network(target(account, project)), 60_000);
export const dnsZonesR = gcp.resource("dns", (account: string, project: string) => gcpApi.dnsZones(target(account, project)), 5 * 60_000);
export const dnsRecordsR = gcp.resource(
    "dns.records",
    (account: string, project: string, zone: string) => gcpApi.dnsRecords(target(account, project), zone),
    60_000,
);
export const alertingR = gcp.resource("alerting", (account: string, project: string) => gcpApi.alerting(target(account, project)), 60_000);

/** Reads the console again, leaving the sign-in alone: only a call that fails re-reads that. */
export const refreshGcp = () => invalidate((kind) => gcp.owns(kind) && kind !== gcpStatusR.kind);
