import { resource } from "../../plugin-api/resources";
import {
    vercelApi,
    type VercelDeployment,
    type VercelDomain,
    type VercelLog,
    type VercelProject,
    type VercelStatus,
    type VercelVariable,
} from "./api";

export const vercelStatusR = resource({
    kind: "vercel.status",
    fetch: (): Promise<VercelStatus> => vercelApi.status(),
    staleAfterMs: 60_000,
});

export const vercelProjectsR = resource({
    kind: "vercel.projects",
    fetch: (team: string): Promise<VercelProject[]> => vercelApi.projects(team),
    staleAfterMs: 30_000,
});

export const vercelDeploymentsR = resource({
    kind: "vercel.deployments",
    fetch: (team: string, project: string): Promise<VercelDeployment[]> => vercelApi.deployments(team, project),
    staleAfterMs: 15_000,
});

export const vercelLogR = resource({
    kind: "vercel.log",
    fetch: (team: string, id: string): Promise<VercelLog> => vercelApi.logs(team, id),
    staleAfterMs: 10_000,
});

export const vercelDomainsR = resource({
    kind: "vercel.domains",
    fetch: (team: string, project: string): Promise<VercelDomain[]> => vercelApi.domains(team, project),
    staleAfterMs: 60_000,
});

export const vercelEnvR = resource({
    kind: "vercel.env",
    fetch: (team: string, project: string): Promise<VercelVariable[]> => vercelApi.env(team, project),
    staleAfterMs: 60_000,
});
