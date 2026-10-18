import { resource } from "../../plugin-api/resources";
import { linearApi, type LinearIssue, type LinearIssueDetail, type LinearStatus } from "./api";

export const linearStatusR = resource({
    kind: "linear.status",
    fetch: (): Promise<LinearStatus> => linearApi.status(),
    staleAfterMs: 60_000,
});

export const linearIssuesR = resource({
    kind: "linear.issues",
    fetch: (workspace: string, query: string): Promise<LinearIssue[]> => linearApi.search(workspace, query),
    staleAfterMs: 30_000,
});

export const linearIssueR = resource({
    kind: "linear.issue",
    fetch: (workspace: string, id: string): Promise<LinearIssueDetail> => linearApi.issue(workspace, id),
    staleAfterMs: 20_000,
});
