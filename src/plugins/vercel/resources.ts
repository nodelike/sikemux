import { defineService } from "../../plugin-api/service";
import { vercelApi } from "./api";
import { VERCEL_PLUGIN } from "./kinds";
import type { VercelDeployments, VercelDomains, VercelOpenLog, VercelProjects, VercelVariables } from "./types.generated";

export const vercel = defineService(VERCEL_PLUGIN, vercelApi.status);

/** The team's projects, kept by the core for the pane. */
export const vercelProjectsV = vercel.view<[team: string], VercelProjects>("projectsView", (team) => ({ team }));

/** A project's latest deployments, kept by the core. */
export const vercelDeploymentsV = vercel.view<[team: string, project: string], VercelDeployments>("deploymentsView", (team, project) => ({
    team,
    project,
}));

/** A deployment with the end of its build log and what an agent is handed of both, kept by the core. */
export const vercelLogV = vercel.view<[team: string, id: string], VercelOpenLog>("logView", (team, id) => ({ team, id }));

/** A project's domains, kept by the core. */
export const vercelDomainsV = vercel.view<[team: string, project: string], VercelDomains>("domainsView", (team, project) => ({ team, project }));

/** The names of a project's environment variables, kept by the core. */
export const vercelEnvV = vercel.view<[team: string, project: string], VercelVariables>("envView", (team, project) => ({ team, project }));
