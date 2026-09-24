import { resource } from "../../plugin-api/resources";
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
} from "./api";

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
