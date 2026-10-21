import { defineService } from "../../plugin-api/service";
import { linearApi } from "./api";
import { LINEAR_PLUGIN } from "./kinds";

export const linear = defineService(LINEAR_PLUGIN, linearApi.status);

export const linearIssuesR = linear.resource("issues", (workspace: string, query: string) => linearApi.search(workspace, query), 30_000);

export const linearIssueR = linear.resource("issue", (workspace: string, id: string) => linearApi.issue(workspace, id), 20_000);
