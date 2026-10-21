import { pluginResources } from "../../plugin-api/service";
import { awsApi } from "./api";
import { AWS_PLUGIN } from "./kinds";

export const aws = pluginResources(AWS_PLUGIN);

export const awsProfilesR = aws.resource("profiles", () => awsApi.profiles());

export const awsAgentProfileR = aws.resource("agentProfile", (project: string | null) => awsApi.agentProfilePicks(project));

export const awsSetupR = aws.resource("setup", () => awsApi.setup());

export const awsIdentityR = aws.resource("status", (profile: string, force: boolean) => awsApi.identity(profile, force), 60_000);

export const ecsClustersR = aws.resource("ecs.clusters", (profile: string) => awsApi.ecsClusters(profile), 30_000);

export const ecsServicesR = aws.resource("ecs.services", (profile: string, cluster: string) => awsApi.ecsServices(profile, cluster), 30_000);

export const ecsTasksR = aws.resource(
    "ecs.tasks",
    (profile: string, cluster: string, service: string) => awsApi.ecsTasks(profile, cluster, service),
    15_000,
);

export const ecsServiceLogConfigR = aws.resource("ecs.serviceLogConfig", (profile: string, cluster: string, service: string) =>
    awsApi.ecsServiceLogConfig(profile, cluster, service),
);

export const ec2InstancesR = aws.resource("ec2.instances", (profile: string) => awsApi.ec2Instances(profile), 60_000);

export const lambdaFnsR = aws.resource("lambda.functions", (profile: string) => awsApi.lambdaFunctions(profile), 60_000);

export const sqsQueuesR = aws.resource("sqs.queues", (profile: string) => awsApi.sqsQueues(profile), 60_000);

export const billingMonthsR = aws.resource(
    "billing.months",
    (profile: string, monthsBack: number) => awsApi.billingMonths(profile, monthsBack),
    5 * 60_000,
);

export const s3BucketsR = aws.resource("s3.buckets", (profile: string) => awsApi.s3Buckets(profile), 5 * 60_000);
