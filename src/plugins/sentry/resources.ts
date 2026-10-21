import { defineService } from "../../plugin-api/service";
import { sentryApi, type IssueQuery, type Project } from "./api";
import { SENTRY_PLUGIN } from "./kinds";

export const sentry = defineService(SENTRY_PLUGIN, sentryApi.status);

export const sentryOrganizationsR = sentry.resource("organizations", () => sentryApi.organizations(), 300_000);

/** Each read below names the organisation, so switching back shows what was read there. */
export const sentryProjectsR = sentry.resource("projects", (_org: string) => sentryApi.projects(), 300_000);

export const sentryMembersR = sentry.resource("members", (_org: string) => sentryApi.members(), 300_000);

export const sentryEnvironmentsR = sentry.resource(
    "environments",
    (_org: string, project: string | null) => sentryApi.environments(project),
    300_000,
);

export const sentryIssuesR = sentry.resource("issues", (_org: string, query: IssueQuery) => sentryApi.issues(query), 30_000);

export const sentryIssueR = sentry.resource("issue", (_org: string, id: string) => sentryApi.issue(id), 30_000);

export const sentryReleasesR = sentry.resource("releases", (_org: string, project: Project) => sentryApi.releases(project), 60_000);
