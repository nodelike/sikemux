import { defineService } from "../../plugin-api/service";
import { vercelApi } from "./api";
import { VERCEL_PLUGIN } from "./kinds";

export const vercel = defineService(VERCEL_PLUGIN, vercelApi.status);

export const vercelProjectsR = vercel.resource("projects", (team: string) => vercelApi.projects(team), 30_000);

export const vercelDeploymentsR = vercel.resource("deployments", (team: string, project: string) => vercelApi.deployments(team, project), 15_000);

export const vercelLogR = vercel.resource("log", (team: string, id: string) => vercelApi.logs(team, id), 10_000);

export const vercelDomainsR = vercel.resource("domains", (team: string, project: string) => vercelApi.domains(team, project), 60_000);

export const vercelEnvR = vercel.resource("env", (team: string, project: string) => vercelApi.env(team, project), 60_000);

export const vercelSettingsR = vercel.resource("settings", (team: string, project: string) => vercelApi.settings(team, project), 60_000);

export const vercelDomainR = vercel.resource(
    "domain",
    (team: string, project: string, domain: string) => vercelApi.domain(team, project, domain),
    30_000,
);

export const vercelCronsR = vercel.resource("crons", (team: string, project: string) => vercelApi.crons(team, project), 60_000);

export const vercelFirewallR = vercel.resource("firewall", (team: string, project: string) => vercelApi.firewall(team, project), 30_000);

export const vercelProtectionR = vercel.resource("protection", (team: string, project: string) => vercelApi.protection(team, project), 60_000);
