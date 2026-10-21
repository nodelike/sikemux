import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import type { VercelDeployment, VercelLog } from "./api";
import { VERCEL_PROJECTS } from "./kinds";

export type VercelSection = "overview" | "deployments" | "env" | "domains";
export type EnvironmentFilter = "all" | "production" | "preview";
export type StatusFilter = "all" | "READY" | "ERROR" | "BUILDING" | "CANCELED";

export interface VercelView {
    /** The project open, by name; none shows every project. */
    project: string | null;
    section: VercelSection;
    /** The deployment open, by id. */
    deployment: string | null;
    environment: EnvironmentFilter;
    status: StatusFilter;
    /** Words in the sidebar's Find box. */
    find: string;
    /** The sidebar's width as the person dragged it; none for the default. */
    sideWidth: number | null;
}

const views = definePaneViews<VercelView>({
    project: null,
    section: "overview",
    deployment: null,
    environment: "all",
    status: "all",
    find: "",
    sideWidth: null,
});

export const useVercelView = views.use;
export const updateVercelView = views.update;

export const openVercel = (): string | null => openSurface(VERCEL_PROJECTS);

/** Still on its way, so its state and log are worth asking for again. */
export const isUnderway = (state: string) => state === "BUILDING" || state === "QUEUED" || state === "INITIALIZING";

/** How often a deployment still on its way is read again, so its state and log keep up. */
export const UNDERWAY_REFRESH_MS = 4_000;

const LOG_LINES_FOR_AGENT = 120;

/** A deployment as an agent reads it: what it is, where it came from, and the end of its build log. */
export function deploymentForAgent(deployment: VercelDeployment, log: VercelLog | undefined): string {
    const facts = [
        `State: ${deployment.state}`,
        `Target: ${deployment.target}`,
        deployment.branch ? `Branch: ${deployment.branch}` : null,
        deployment.commit ? `Commit: ${deployment.commit}${deployment.message ? ` — ${deployment.message}` : ""}` : null,
        deployment.author ? `By: ${deployment.author}` : null,
        deployment.error ? `Error: ${deployment.error}` : null,
    ].filter(Boolean);
    const lines = log?.lines.slice(-LOG_LINES_FOR_AGENT).map((line) => line.text) ?? [];
    return [
        `Vercel deployment ${deployment.id} of ${deployment.project}${deployment.url ? ` — ${deployment.url}` : ""}`,
        facts.join("\n"),
        lines.length > 0 ? `End of the build log:\n\n\`\`\`\n${lines.join("\n")}\n\`\`\`` : null,
        `To read more, use vercel_logs or vercel_deployment with id ${deployment.id}.`,
    ]
        .filter(Boolean)
        .join("\n\n");
}
