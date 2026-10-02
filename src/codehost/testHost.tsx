import type { ReactNode } from "react";
import { HostProvider, registerCodeHost, type CodeHost, type CodeHostApi } from "./registry";

export const TEST_HOST = "test.host";

/** A code host whose API is whatever a test hands it, registered once per test file. */
export function registerTestHost(api: object): CodeHost {
    const host: CodeHost = {
        id: TEST_HOST,
        name: "Test host",
        ciName: "CI",
        icon: () => null,
        capabilities: {
            ci: {
                graph: true,
                attempts: true,
                approvals: true,
                dispatch: true,
                annotations: true,
                summaries: true,
                artifacts: true,
                billing: true,
                workflowFile: true,
                rerunFailed: true,
                rerunJob: true,
                debugLogs: true,
                deleteRuns: true,
            },
            pulls: { draft: true, mergeMethods: ["squash", "merge", "rebase"], requestChanges: true, reopen: true, mergeability: true },
            issues: true,
            releases: true,
            inbox: true,
        },
        api: api as unknown as CodeHostApi,
        SignIn: () => null,
    };
    registerCodeHost(host);
    return host;
}

export function InHost({ host, children }: { host: CodeHost; children: ReactNode }) {
    return <HostProvider value={host}>{children}</HostProvider>;
}
