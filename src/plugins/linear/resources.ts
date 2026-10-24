import { defineService } from "../../plugin-api/service";
import { linearApi } from "./api";
import { LINEAR_PLUGIN } from "./kinds";
import type { LinearIssueList, LinearIssueView } from "./types.generated";

export const linear = defineService(LINEAR_PLUGIN, linearApi.status);

/** The person's open issues, or those `query` finds, kept by the core. */
export const linearListV = linear.view<[workspace: string, query: string], LinearIssueList>("listView", (workspace, query) => ({
    workspace,
    query,
}));

/** One issue with its team's states and its comments, kept by the core. */
export const linearIssueV = linear.view<[workspace: string, id: string], LinearIssueView>("issueView", (workspace, id) => ({ workspace, id }));
