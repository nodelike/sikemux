import type { ReactNode } from "react";
import { installIpcTransportForTests, MemoryIpcTransport } from "../api/transport";
import { HostProvider, registerCodeHost, type CodeHost, type CodeHostApi } from "./registry";
import type { CheckTally, Pull, Release, RepoRef, Run } from "./types";
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

/* The test host's reads the stand-in core lists from, each the plugin method the core asks. */
type Reads = Partial<Record<"pulls" | "pull" | "runs" | "issues" | "releases", (...args: never[]) => Promise<unknown>>>;

const READINESS: Record<string, string> = {
    dirty: "conflicts",
    blocked: "blocked",
    behind: "behind",
    clean: "ready",
    unstable: "ready",
    has_hooks: "ready",
};

/** A pull request as the core lists it, with checks a test gives it by number. */
function listed(pull: Pull, checks: (pull: Pull) => CheckTally | null) {
    const state = pull.state === "merged" || pull.mergedAt ? "merged" : pull.state === "open" && pull.draft ? "draft" : pull.state;
    return { pull, state, closes: [], checks: checks(pull) };
}

async function listFrom(reads: Reads, method: string, params: Record<string, unknown>, checks: (pull: Pull) => CheckTally | null) {
    const { project: _project, ...query } = params;
    const repo = { provider: TEST_HOST, owner: "", name: "" };
    const read = (name: keyof Reads, ...args: unknown[]) => (reads[name] as (...args: unknown[]) => Promise<never>)(...args);
    if (method === "codeHostPulls")
        return { pulls: ((await read("pulls", repo, query.state)) as Pull[]).map((pull) => listed(pull, checks)), counts: null };
    if (method === "codeHostRuns") {
        const page = (await read("runs", { ...repo, ...query })) as { runs: Run[]; total: number; nextPage?: number | null };
        return {
            runs: page.runs.map((run) => ({ run, unfinished: run.status !== "completed" })),
            total: page.total,
            nextPage: page.nextPage ?? null,
        };
    }
    if (method === "codeHostIssues") return { ...((await read("issues", repo, query.state, query.page)) as object), counts: null };
    if (method === "codeHostPull") {
        const pull = (await read("pull", repo, query.number)) as Pull;
        const runs = pull.headSha && reads.runs ? ((await read("runs", { ...repo, headSha: pull.headSha })) as { runs: Run[] }).runs : [];
        return {
            ...listed(pull, checks),
            ownBranch: true,
            verdict: null,
            reviewed: false,
            merge: READINESS[pull.mergeState ?? ""] ?? "checking",
            checkout: (pull as Pull & { checkout?: string | null }).checkout ?? null,
            runs: runs.map((run) => ({ run, unfinished: run.status !== "completed" })),
        };
    }
    if (method === "codeHostReleases") {
        const releases = (await read("releases", repo)) as Release[];
        return { releases, latest: releases.find((release) => !release.draft && !release.prerelease)?.id ?? null };
    }
    return undefined;
}

/** What the stand-in core answers a call with outright, by the call's method. */
type Answers = Record<string, (params: Record<string, unknown>) => unknown>;

/**
 * A stand-in core, once per test file. It keeps the repositories the page asks it to check CI for and can say a
 * repository's CI moved, and lists a project's pull requests, runs, issues and releases from the test host's reads,
 * or answers a call from `answers`.
 */
export function ciCore(reads: Reads = {}, checks: (pull: Pull) => CheckTally | null = () => null, answers: Answers = {}) {
    const transport = new MemoryIpcTransport();
    const shown: unknown[] = [];
    transport.register("core_call", async (args) => {
        const call = args as { method?: string; params?: Record<string, unknown> } | undefined;
        if (call?.method === "showCi") shown.push(call.params);
        const answer = call?.method ? answers[call.method] : undefined;
        if (answer) return { value: await answer(call?.params ?? {}) };
        const list = call?.method ? await listFrom(reads, call.method, call.params ?? {}, checks) : undefined;
        return list === undefined ? null : { value: list };
    });
    installIpcTransportForTests(transport);
    return {
        shown,
        moved: (repo: RepoRef) =>
            transport.emit("core_changed", { topic: "ci", project: null, paths: [`${repo.provider}:${repo.owner}/${repo.name}`] }),
        /** The core read lists again and found them changed: those named, or every one. */
        listsMoved: (names: string[] | null = null) => transport.emit("core_changed", { topic: "codeHost", project: null, paths: names }),
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
