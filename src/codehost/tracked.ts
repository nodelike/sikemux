import { useMemo } from "react";
import type { PromptContext } from "../api/acp";
import { useResourceEnabled } from "../plugin-api/resources";
import { failureMessage } from "./api";
import { useHostRepo } from "./project";
import { codeHost, hostApi } from "./registry";
import { hostStatusR, issuesR, pullsR } from "./resources";
import type { Comment, Issue, Pull, RepoRef } from "./types";

export type TrackedKind = "issue" | "pull";

/** An open issue or pull request a message can be about. */
export interface TrackedItem {
    kind: TrackedKind;
    number: number;
    title: string;
    createdAt: string;
    url: string;
}

const tracked = (kind: TrackedKind, item: Issue | Pull): TrackedItem => ({
    kind,
    number: item.number,
    title: item.title,
    createdAt: item.createdAt,
    url: item.url,
});

/** Issues and pull requests together, newest first. A host that lists pull requests among its issues has each kept once, as a pull request. */
export function trackedItems(issues: readonly Issue[], pulls: readonly Pull[]): TrackedItem[] {
    const pullNumbers = new Set(pulls.map((pull) => pull.number));
    return [
        ...pulls.map((pull) => tracked("pull", pull)),
        ...issues.filter((issue) => !pullNumbers.has(issue.number)).map((issue) => tracked("issue", issue)),
    ].sort((a, b) => b.createdAt.localeCompare(a.createdAt) || b.number - a.number);
}

/** Digits match the start of a number; words must all appear in the title. */
export function filterTracked(items: readonly TrackedItem[], needle: string, limit: number): TrackedItem[] {
    const query = needle.trim().toLowerCase();
    if (/^\d+$/.test(query)) return items.filter((item) => String(item.number).startsWith(query)).slice(0, limit);
    const words = query.split(/\s+/).filter(Boolean);
    return items.filter((item) => words.every((word) => item.title.toLowerCase().includes(word))).slice(0, limit);
}

export type TrackedList = { state: "loading" } | { state: "unavailable"; message: string } | { state: "ready"; repo: RepoRef; items: TrackedItem[] };

/** The open issues and pull requests of the repository a project's remote points at, once someone is signed in to its host. */
export function useTrackedItems(cwd: string, enabled: boolean): TrackedList {
    const found = useHostRepo(cwd || null, enabled && !!cwd);
    const repo = found.repo;
    const host = repo ? codeHost(repo.provider) : undefined;
    const status = useResourceEnabled(enabled && !!repo, hostStatusR, repo?.provider ?? "", repo?.account ?? null);
    const signedIn = !!status.data?.ok;
    const withIssues = !!host?.capabilities.issues;
    const listed = repo ?? { provider: "", owner: "", name: "" };
    const issues = useResourceEnabled(enabled && signedIn && withIssues, issuesR, listed, "open", 1);
    const pulls = useResourceEnabled(enabled && signedIn, pullsR, listed, "open");

    const error = pulls.error ?? (withIssues ? issues.error : null);
    return useMemo((): TrackedList => {
        if (!cwd) return { state: "unavailable", message: "Open a project to pick its issues" };
        if (found.loading) return { state: "loading" };
        if (!repo || !host) return { state: "unavailable", message: "This project has no remote on a code host" };
        if (!status.data) return status.error ? { state: "unavailable", message: failureMessage(status.error) } : { state: "loading" };
        if (!signedIn) return { state: "unavailable", message: `Sign in to ${host.name} in the Git pane to pick from its issues` };
        if (error) return { state: "unavailable", message: failureMessage(error) };
        if (!pulls.data || (withIssues && !issues.data)) return { state: "loading" };
        return { state: "ready", repo, items: trackedItems(issues.data?.issues ?? [], pulls.data) };
    }, [cwd, found.loading, repo, host, status.data, status.error, signedIn, error, pulls.data, withIssues, issues.data]);
}

const MAX_BODY = 12_000;
const MAX_COMMENT = 2_000;
const LATEST_COMMENTS = 5;

const clip = (text: string, limit: number) =>
    text.length > limit ? `${text.slice(0, limit).trimEnd()}\n\n[… cut, ${text.length - limit} more characters]` : text;

const day = (at: string) => at.slice(0, 10);

/** An issue or pull request as text an agent can work from: what it is, what it says, and the latest of what was said about it. */
export function trackedContext(kind: TrackedKind, item: Issue | Pull, comments: readonly Comment[]): PromptContext {
    const noun = kind === "issue" ? "Issue" : "Pull request";
    const pull = kind === "pull" ? (item as Pull) : null;
    const facts = [
        `State: ${pull?.mergedAt ? "merged" : pull?.draft ? "draft" : item.state}`,
        item.labels.length > 0 ? `Labels: ${item.labels.map((label) => label.name).join(", ")}` : null,
        item.author ? `Author: @${item.author}` : null,
        pull?.head && pull.base ? `Branch: ${pull.head} → ${pull.base}` : null,
    ].filter(Boolean);
    const latest = comments.slice(-LATEST_COMMENTS);
    const lines = [
        `${noun} #${item.number}: ${item.title}`,
        facts.join(" · "),
        `URL: ${item.url}`,
        "",
        clip(item.body.trim() || "(no description)", MAX_BODY),
    ];
    if (latest.length > 0) {
        lines.push("", latest.length < comments.length ? `## Latest ${latest.length} of ${comments.length} comments` : "## Comments");
        for (const comment of latest)
            lines.push("", `@${comment.author ?? "someone"}, ${day(comment.createdAt)}:`, clip(comment.body.trim(), MAX_COMMENT));
    }
    return { uri: item.url, title: `#${item.number} ${item.title}`, text: lines.join("\n") };
}

/** Reads an issue or pull request in full, for the moment a message about it is sent. */
export async function loadTrackedContext(repo: RepoRef, kind: TrackedKind, number: number): Promise<PromptContext> {
    const api = hostApi(repo.provider);
    const [item, comments] = await Promise.all([
        kind === "issue" ? api.issue(repo, number) : api.pull(repo, number),
        api.comments(repo, number).catch((): Comment[] => []),
    ]);
    return trackedContext(kind, item, comments);
}
