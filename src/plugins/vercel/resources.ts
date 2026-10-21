import { defineService } from "../../plugin-api/service";
import { vercelApi } from "./api";
import { VERCEL_PLUGIN } from "./kinds";

export const vercel = defineService(VERCEL_PLUGIN, vercelApi.status);

export const vercelProjectsR = vercel.resource("projects", (team: string) => vercelApi.projects(team), 30_000);

export const vercelDeploymentsR = vercel.resource("deployments", (team: string, project: string) => vercelApi.deployments(team, project), 15_000);

export const vercelLogR = vercel.resource("log", (team: string, id: string) => vercelApi.logs(team, id), 10_000);

export const vercelDomainsR = vercel.resource("domains", (team: string, project: string) => vercelApi.domains(team, project), 60_000);

export const vercelEnvR = vercel.resource("env", (team: string, project: string) => vercelApi.env(team, project), 60_000);
