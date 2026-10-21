import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { VERCEL_PLUGIN } from "./kinds";

const backend = createPluginBackend(VERCEL_PLUGIN.id);

export interface VercelAccount {
    id: string;
    username: string;
    name: string;
    email: string;
    /** A browser sign-in sees only the teams the Sikemux integration is installed on. */
    signIn: "token" | "browser";
}

export interface VercelTeam {
    id: string;
    slug: string;
    name: string;
}

export interface VercelStatus {
    configured: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    account: VercelAccount | null;
    teams: VercelTeam[];
    /** The team calls are for, by id; null for the account's own projects. */
    team: string | null;
    /** Whether the account has projects outside its teams; newer accounts keep them in a default team. */
    personal: boolean;
    /** Whether this build can sign in through the browser. */
    browserSignIn: boolean;
}

/** BUILDING, ERROR, INITIALIZING, QUEUED, READY or CANCELED. */
export type DeploymentState = string;

export interface VercelProject {
    id: string;
    name: string;
    framework: string | null;
    updatedAt: number;
    repo: string | null;
    gitProvider: string | null;
    productionBranch: string | null;
    /** The deployment serving production. */
    production: { id: string; url: string | null; state: DeploymentState; createdAt: number } | null;
    /** A newer production build that is still building or failed, while the one before it serves. */
    latestProduction: { id: string; state: DeploymentState; createdAt: number } | null;
}

export interface VercelDeployment {
    id: string;
    project: string;
    projectId: string | null;
    url: string | null;
    state: DeploymentState;
    target: "production" | "preview";
    createdAt: number;
    readyAt: number | null;
    /** How long the build ran, once it finished. */
    buildMs: number | null;
    branch: string | null;
    commit: string | null;
    message: string | null;
    author: string | null;
    /** The deployment this one built again, for a redeploy. */
    redeployOf: string | null;
    inspectorUrl: string | null;
    aliases: string[] | null;
    error: string | null;
}

export interface VercelLogLine {
    at: number;
    /** stdout, stderr, command or fatal. */
    kind: string;
    text: string;
}

export interface VercelLog {
    deployment: VercelDeployment;
    lines: VercelLogLine[];
    skipped: number;
}

export interface VercelDomain {
    name: string;
    verified: boolean;
    redirect: string | null;
    gitBranch: string | null;
}

export interface VercelVariable {
    id: string;
    key: string;
    target: string[];
    type: string;
    gitBranch: string | null;
    updatedAt: number | null;
}

/** Every call names its team, so the pane never acts on another team than the one it shows. */
export const vercelApi = {
    status: () => backend.call<VercelStatus>("status"),
    signIn: (token: string) => backend.call<VercelStatus>("signIn", { token }),
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<VercelStatus>(backend, "signInWithBrowser", openPage),
    signOut: () => backend.call<void>("signOut"),
    setTeam: (team: string) => backend.call<void>("setTeam", { team }),
    projects: (team: string) => backend.call<VercelProject[]>("projects", { team }),
    deployments: (team: string, project: string) => backend.call<VercelDeployment[]>("deployments", { team, project, limit: 50 }),
    deployment: (team: string, id: string) => backend.call<VercelDeployment>("deployment", { team, id }),
    logs: (team: string, id: string) => backend.call<VercelLog>("logs", { team, id, tail: 2000 }),
    domains: (team: string, project: string) => backend.call<VercelDomain[]>("domains", { team, project }),
    env: (team: string, project: string) => backend.call<VercelVariable[]>("env", { team, project }),
    redeploy: (team: string, id: string, target?: "production") => backend.call<VercelDeployment>("redeploy", { team, id, target }),
    promote: (team: string, id: string) => backend.call<VercelDeployment>("promote", { team, id }),
    cancel: (team: string, id: string) => backend.call<VercelDeployment>("cancel", { team, id }),
};
