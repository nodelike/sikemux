import { agentApi, type AgentInfo, type AgentModelInfo, type AgentSession, type AgentUsage } from "../api/agents";
import type { AgentRuntimeProfile } from "../agentProfiles";
import {
    awsApi,
    type AwsIdentity,
    type AwsProfile,
    type BillingMonth,
    type Ec2Instance,
    type EcsCluster,
    type EcsService,
    type EcsServiceLog,
    type EcsTask,
    type LambdaFn,
    type S3Bucket,
    type SqsQueue,
} from "../api/aws";
import { filesApi } from "../api/files";
import { git, type DiscoveredRepo, type GitOverview, type GitRemote, type GitRemoteBranch, type GitStash } from "../api/git";
import {
    rundeckApi,
    type MatrixResult,
    type PlanResult,
    type RundeckEnvSpec,
    type RundeckExecution,
    type RundeckJob,
    type RundeckProject,
    type RundeckStatus,
} from "../api/rundeck";
import { settingsApi, type ProjectEntry } from "../api/settings";
import { loadCollection } from "../bruno/collection";
import type { BruCollection } from "../bruno/types";
import { sshApi, type SshHost } from "../api/ssh";
import type { AgentType, ProjectRoot } from "./types";
import { resource } from "./resources";

export const gitOverviewR = resource({
    kind: "git.overview",
    fetch: (repo: string): Promise<GitOverview> => git.overview(repo),
    staleAfterMs: 5_000,
});

export const gitDiscoveredReposR = resource({
    kind: "git.discoveredRepos",
    fetch: (root: string): Promise<DiscoveredRepo[]> => git.discoverRepos(root),
    staleAfterMs: 5_000,
});

export const gitRemotesR = resource({
    kind: "git.remotes",
    fetch: (repo: string): Promise<GitRemote[]> => git.remotes(repo),
    staleAfterMs: 5 * 60_000,
});

export const gitRemoteBranchesR = resource({
    kind: "git.remoteBranches",
    fetch: (repo: string, remote: string): Promise<GitRemoteBranch[]> => git.remoteBranches(repo, remote),
    staleAfterMs: 30_000,
});

export const gitStashesR = resource({
    kind: "git.stashes",
    fetch: (repo: string): Promise<GitStash[]> => git.stashList(repo),
    staleAfterMs: 30_000,
});

export const awsProfilesR = resource({
    kind: "aws.profiles",
    fetch: (): Promise<AwsProfile[]> => awsApi.profiles(),
});

export const awsIdentityR = resource({
    kind: "aws.identity",
    fetch: (profile: string, force: boolean): Promise<AwsIdentity> => awsApi.identity(profile, force),
    staleAfterMs: 60_000,
});

export const ecsClustersR = resource({
    kind: "aws.ecs.clusters",
    fetch: (profile: string): Promise<EcsCluster[]> => awsApi.ecsClusters(profile),
    staleAfterMs: 30_000,
});

export const ecsServicesR = resource({
    kind: "aws.ecs.services",
    fetch: (profile: string, cluster: string): Promise<EcsService[]> => awsApi.ecsServices(profile, cluster),
    staleAfterMs: 30_000,
});

export const ecsTasksR = resource({
    kind: "aws.ecs.tasks",
    fetch: (profile: string, cluster: string, service: string): Promise<EcsTask[]> => awsApi.ecsTasks(profile, cluster, service),
    staleAfterMs: 15_000,
});

export const ecsServiceLogConfigR = resource({
    kind: "aws.ecs.serviceLogConfig",
    fetch: (profile: string, cluster: string, service: string): Promise<EcsServiceLog> => awsApi.ecsServiceLogConfig(profile, cluster, service),
});

export const ec2InstancesR = resource({
    kind: "aws.ec2.instances",
    fetch: (profile: string): Promise<Ec2Instance[]> => awsApi.ec2Instances(profile),
    staleAfterMs: 60_000,
});

export const lambdaFnsR = resource({
    kind: "aws.lambda.functions",
    fetch: (profile: string): Promise<LambdaFn[]> => awsApi.lambdaFunctions(profile),
    staleAfterMs: 60_000,
});

export const sqsQueuesR = resource({
    kind: "aws.sqs.queues",
    fetch: (profile: string): Promise<SqsQueue[]> => awsApi.sqsQueues(profile),
    staleAfterMs: 60_000,
});

export const billingMonthsR = resource({
    kind: "aws.billing.months",
    fetch: (profile: string, monthsBack: number): Promise<BillingMonth[]> => awsApi.billingMonths(profile, monthsBack),
    staleAfterMs: 5 * 60_000,
});

export const s3BucketsR = resource({
    kind: "aws.s3.buckets",
    fetch: (profile: string): Promise<S3Bucket[]> => awsApi.s3Buckets(profile),
    staleAfterMs: 5 * 60_000,
});

export const agentCatalogR = resource({
    kind: "agents.catalog",
    fetch: (profiles: AgentRuntimeProfile[]): Promise<AgentInfo[]> => agentApi.available(profiles),
    staleAfterMs: 60_000,
});

export const agentModelsR = resource({
    kind: "agents.models",
    fetch: (type: AgentType, executablePath?: string, configPath?: string): Promise<AgentModelInfo[]> =>
        agentApi.models(type, executablePath, configPath),
    staleAfterMs: 5 * 60_000,
});

export const agentUsageR = resource({
    kind: "agents.usage",
    fetch: (type: AgentType, executablePath?: string, configPath?: string): Promise<AgentUsage> => agentApi.usage(type, executablePath, configPath),
    staleAfterMs: 5 * 60_000,
});

export const agentSessionsR = resource({
    kind: "agents.sessions",
    fetch: (type: AgentType, cwd: string, configPath?: string): Promise<AgentSession[]> => agentApi.sessions(type, cwd, configPath),
    staleAfterMs: 0,
});

export const filesListR = resource({
    kind: "files.list",
    fetch: (repo: string): Promise<string[]> => filesApi.list(repo),
    staleAfterMs: 60_000,
});

export const projectRootsScanR = resource({
    kind: "settings.projectRootsScan",
    fetch: (roots: ProjectRoot[]): Promise<ProjectEntry[]> => settingsApi.scanProjectRoots(roots),
    staleAfterMs: 60_000,
});

export const sshHostsR = resource({
    kind: "ssh.hosts",
    fetch: (): Promise<SshHost[]> => sshApi.hosts(),
    staleAfterMs: 5 * 60_000,
});

export const rndStatusR = resource({
    kind: "rnd.status",
    fetch: (): Promise<RundeckStatus> => rundeckApi.status(),
    staleAfterMs: 60_000,
});

export const rndProjectsR = resource({
    kind: "rnd.projects",
    fetch: (): Promise<RundeckProject[]> => rundeckApi.projects(),
    staleAfterMs: 5 * 60_000,
});

export const rndJobsR = resource({
    kind: "rnd.jobs",
    fetch: (project: string): Promise<RundeckJob[]> => rundeckApi.jobs(project),
    staleAfterMs: 60_000,
});

export const rndMatrixR = resource({
    kind: "rnd.matrix",
    fetch: (envs: RundeckEnvSpec[]): Promise<MatrixResult> => rundeckApi.branchesMatrix(envs),
    staleAfterMs: 30_000,
});

export const rndExecutionsR = resource({
    kind: "rnd.executions",
    fetch: (jobId: string, project: string, max: number): Promise<RundeckExecution[]> => rundeckApi.executions(jobId, project, max),
    staleAfterMs: 15_000,
});

export const rndPlanR = resource({
    kind: "rnd.plan",
    fetch: (project: string, service: string, branch: string, repoPath: string): Promise<PlanResult> =>
        rundeckApi.plan(project, service, branch, repoPath),
    staleAfterMs: 10_000,
});

export const brunoCollectionR = resource({
    kind: "bruno.collection",
    fetch: (rootPath: string): Promise<BruCollection> => loadCollection(rootPath),
    staleAfterMs: 5 * 60_000,
});
