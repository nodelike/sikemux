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

export type { VercelDeployment, VercelDomain, VercelLog, VercelLogLine, VercelProject, VercelVariable } from "./types.generated";
import type { VercelDeployment } from "./types.generated";

/** Every call names its team, so the pane never acts on another team than the one it shows. */
export const vercelApi = {
    status: () => backend.call<VercelStatus>("status"),
    signIn: (token: string) => backend.call<VercelStatus>("signIn", { token }),
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<VercelStatus>(backend, "signInWithBrowser", openPage),
    signOut: () => backend.call<void>("signOut"),
    setTeam: (team: string) => backend.call<void>("setTeam", { team }),
    redeploy: (team: string, id: string, target?: "production") => backend.call<VercelDeployment>("redeploy", { team, id, target }),
    promote: (team: string, id: string) => backend.call<VercelDeployment>("promote", { team, id }),
    cancel: (team: string, id: string) => backend.call<VercelDeployment>("cancel", { team, id }),
};
