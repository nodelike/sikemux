import type { ReactNode } from "react";
import { installIpcTransportForTests, MemoryIpcTransport } from "../api/transport";
import { HostProvider, registerCodeHost, type CodeHost, type CodeHostApi } from "./registry";
import type { RepoRef } from "./types";
import { getState } from "../state/store";

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
                deleteLogs: true,
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

/** A stand-in core, once per test file, that keeps the repositories the page asks it to check CI for and can say a repository's CI moved. */
export function ciCore() {
    const transport = new MemoryIpcTransport();
    const shown: unknown[] = [];
    transport.register("core_call", (args) => {
        const call = args as { method?: string; params?: unknown } | undefined;
        if (call?.method === "showCi") shown.push(call.params);
        return null;
    });
    installIpcTransportForTests(transport);
    return {
        shown,
        moved: (repo: RepoRef) =>
            transport.emit("core_changed", { topic: "ci", project: null, paths: [`${repo.provider}:${repo.owner}/${repo.name}`] }),
    };
}

/** What each project folder picked on `provider`, as `repo` or `account`, from the core's workspace in the store. */
export function picked(provider: string, key: "repo" | "account"): Record<string, string> {
    return Object.fromEntries(
        Object.entries(getState().codeHosts).flatMap(([cwd, hosts]) => {
            const value = hosts[provider]?.[key];
            return value ? [[cwd, value]] : [];
        }),
    );
}
