import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { AWS_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(AWS_PLUGIN_ID);

const SIGNED_OUT = new Set(["aws-token-expired", "aws-no-credentials", "aws-cli-missing"]);

let onSignedOut: (profile: string) => void = () => {};

/** Called when a request finds the profile signed out, so the plugin can drop what it has cached. */
export function whenSignedOut(listener: (profile: string) => void): void {
    onSignedOut = listener;
}

async function call<T>(method: string, params: Record<string, unknown>): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isPluginFailure(error) && SIGNED_OUT.has(error.category) && typeof params.profile === "string") onSignedOut(params.profile);
        throw error;
    }
}

export interface AwsProfile {
    name: string;
    region: string | null;
    sso_start_url: string | null;
    sso_region: string | null;
    sso_account_id: string | null;
    sso_role_name: string | null;
    kind: string;
}

export type AwsStatus = "authed" | "expired" | "no-credentials" | "error" | "cli-missing" | "unknown" | "checking";

export interface AwsIdentity {
    arn: string | null;
    account: string | null;
    user_id: string | null;
    status: Exclude<AwsStatus, "unknown" | "checking">;
    message: string | null;
}

export interface AwsLoginResult {
    success: boolean;
    stdout: string;
    stderr: string;
}

export interface EcsCluster {
    name: string;
    arn: string;
    services_count: number | null;
    tasks_running: number | null;
    tasks_pending: number | null;
    status: string | null;
}

export interface EcsService {
    name: string;
    arn: string;
    desired: number | null;
    running: number | null;
    pending: number | null;
    status: string | null;
    primary_created_at: string | null;
    primary_updated_at: string | null;
}

export interface EcsTask {
    arn: string;
    task_id: string;
    status: string | null;
    desired_status: string | null;
    health_status: string | null;
    cpu: string | null;
    memory: string | null;
    started_at: string | null;
    last_status_change: string | null;
}

export interface Ec2Instance {
    instance_id: string;
    name: string | null;
    state: string | null;
    instance_type: string | null;
    private_ip: string | null;
    public_ip: string | null;
    launch_time: string | null;
}

export interface LambdaFn {
    name: string;
    runtime: string | null;
    last_modified: string | null;
    memory_size: number | null;
    timeout: number | null;
    handler: string | null;
}

export interface SqsQueue {
    name: string;
    url: string;
    messages: string | null;
    in_flight: string | null;
    delayed: string | null;
}

export interface EcsTaskLog {
    log_group: string;
    log_stream: string;
    container_name: string;
    region: string | null;
}

export interface EcsServiceLog {
    log_group: string;
    container_name: string;
    region: string | null;
}

export interface BillingService {
    service: string;
    amount: string;
    unit: string;
}

export interface BillingMonth {
    period_start: string;
    period_end: string;
    total: string;
    unit: string;
    is_current: boolean;
    by_service: BillingService[];
}

export interface S3Bucket {
    name: string;
    created_at: string | null;
}

export interface SsoLogin {
    readonly result: Promise<AwsLoginResult>;
    cancel(): void;
}

export const awsApi = {
    profiles: () => backend.call<AwsProfile[]>("profiles", {}),
    identity: (profile: string, force = false) => backend.call<AwsIdentity>("identity", { profile, force }),
    ecsClusters: (profile: string) => call<EcsCluster[]>("ecsClusters", { profile }),
    ecsServices: (profile: string, cluster: string) => call<EcsService[]>("ecsServices", { profile, cluster }),
    ecsTasks: (profile: string, cluster: string, service: string) => call<EcsTask[]>("ecsTasks", { profile, cluster, service }),
    ecsTaskLogConfig: (profile: string, cluster: string, taskArn: string) => call<EcsTaskLog>("ecsTaskLogConfig", { profile, cluster, taskArn }),
    ecsServiceLogConfig: (profile: string, cluster: string, service: string) =>
        call<EcsServiceLog>("ecsServiceLogConfig", { profile, cluster, service }),
    ec2Instances: (profile: string) => call<Ec2Instance[]>("ec2Instances", { profile }),
    lambdaFunctions: (profile: string) => call<LambdaFn[]>("lambdaFunctions", { profile }),
    sqsQueues: (profile: string) => call<SqsQueue[]>("sqsQueues", { profile }),
    billingMonths: (profile: string, monthsBack = 5) => call<BillingMonth[]>("billingMonths", { profile, monthsBack }),
    s3Buckets: (profile: string) => call<S3Bucket[]>("s3Buckets", { profile }),

    /** Waits for the person to approve in their browser, so it streams rather than calls: a call would time out. */
    ssoLogin(profile: string): SsoLogin {
        let settle: { resolve: (result: AwsLoginResult) => void; reject: (error: unknown) => void } | null = null;
        const result = new Promise<AwsLoginResult>((resolve, reject) => {
            settle = { resolve, reject };
        });
        const finish = (outcome: AwsLoginResult | Error) => {
            const pending = settle;
            settle = null;
            if (!pending) return;
            if (outcome instanceof Error) pending.reject(outcome);
            else pending.resolve(outcome);
        };
        const stream = backend.stream<AwsLoginResult>(
            "ssoLogin",
            { profile },
            {
                onItem: finish,
                onEnd: () => finish(new Error("the sign-in ended without an answer")),
                onError: (error) => finish(new Error(error.message)),
            },
        );
        return {
            result,
            cancel() {
                stream.stop();
                finish(new Error("cancelled"));
            },
        };
    },

    tailLogs: (
        params: { profile: string; logGroup: string; logStream: string | null; since: string },
        handlers: { onLine: (line: string) => void; onEnd: () => void; onError: (message: string) => void },
    ) =>
        backend.stream<string>("tailLogs", params, {
            onItem: handlers.onLine,
            onEnd: handlers.onEnd,
            onError: (error) => handlers.onError(error.message),
        }),
};
