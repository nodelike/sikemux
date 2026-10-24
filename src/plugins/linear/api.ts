import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { LINEAR_PLUGIN } from "./kinds";

const backend = createPluginBackend(LINEAR_PLUGIN.id);

export interface LinearWorkspace {
    id: string;
    name: string;
    urlKey: string;
    user: string;
    isDefault: boolean;
}

export interface LinearStatus {
    configured: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    workspaces: LinearWorkspace[];
}

export interface LinearChange {
    state?: string;
    assignee?: string;
    priority?: number;
}

/** What a comment or a change answers; the pane reads the issue again from its view. */
export interface LinearWritten {
    id: string;
}

export const linearApi = {
    status: () => backend.call<LinearStatus>("status"),
    signIn: (key: string) => backend.call<LinearStatus>("signIn", { key }),
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<LinearStatus>(backend, "signInWithBrowser", openPage),
    signOut: (workspace: string) => backend.call<void>("signOut", { workspace }),
    comment: (workspace: string, id: string, body: string) => backend.call<LinearWritten>("comment", { workspace, id, body }),
    update: (workspace: string, id: string, change: LinearChange) => backend.call<LinearWritten>("update", { workspace, id, ...change }),
};
