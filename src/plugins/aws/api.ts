import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { openSignInUrl, swallow } from "../../plugin-api/host";
import { AWS_PLUGIN } from "./kinds";
import type { AwsLogLine } from "./types.generated";

const backend = createPluginBackend(AWS_PLUGIN.id, {
    signedOutCategories: ["aws-token-expired", "aws-no-credentials", "aws-cli-missing"],
});

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

/** Whether the AWS CLI is installed, and whether Homebrew is there to install it. */
export interface AwsSetup {
    cli: boolean;
    homebrew: boolean;
}

export interface AwsLoginResult {
    success: boolean;
    stdout: string;
    stderr: string;
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

/** Whether AWS calls work as the profile the pane shows. */
export interface AwsSignInStatus {
    readonly configured: boolean;
    readonly ok: boolean;
    readonly authFailed: boolean;
    readonly message: string | null;
    readonly profile: string | null;
    readonly account: string | null;
}

export const awsApi = {
    status: () => backend.call<AwsSignInStatus>("status"),
    /** The profile the pane shows, which the plugin keeps. */
    profile: () => backend.call<{ profile: string | null }>("profile"),
    pickProfile: (profile: string | null) => backend.call<{ profile: string | null }>("setDefaultAccount", { profile }),
    profiles: () => backend.call<AwsProfile[]>("profiles", {}),
    setup: () => backend.call<AwsSetup>("setup", {}),
    pickAgentProfile: (project: string | null, profile: string | null) => backend.call<unknown>("pickAgentProfile", { project, profile }),
    identity: (profile: string, force = false) => backend.call<AwsIdentity>("identity", { profile, force }),
    ecsTaskLogConfig: (profile: string, cluster: string, taskArn: string) =>
        backend.call<EcsTaskLog>("ecsTaskLogConfig", { profile, cluster, taskArn }),
    ecsServiceLogConfig: (profile: string, cluster: string, service: string) =>
        backend.call<EcsServiceLog>("ecsServiceLogConfig", { profile, cluster, service }),

    /** Waits for the person to approve in their browser, so it streams rather than calls: a call would time out. */
    ssoLogin: (profile: string) =>
        browserSignIn<AwsLoginResult>(backend, "signIn", (url) => void openSignInUrl(url).catch(swallow("open AWS")), { profile }),

    tailLogs: (
        params: { profile: string; logGroup: string; logStream: string | null; since: string },
        handlers: { onLine: (line: AwsLogLine) => void; onEnd: () => void; onError: (message: string) => void },
    ) =>
        backend.stream<AwsLogLine>("tailLogs", params, {
            onItem: handlers.onLine,
            onEnd: handlers.onEnd,
            onError: (error) => handlers.onError(error.message),
        }),
};
