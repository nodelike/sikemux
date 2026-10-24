import { createPluginBackend } from "../../plugin-api/backend";
import { SLACK_PLUGIN } from "./kinds";
import type { SlackThread } from "./types.generated";

const backend = createPluginBackend(SLACK_PLUGIN.id);

export interface SlackWorkspace {
    id: string;
    name: string;
    domain: string;
    user: string;
    userId: string;
    isDefault: boolean;
}

export interface SlackStatus {
    configured: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    workspaces: SlackWorkspace[];
}

export interface SlackPosted {
    channel: string;
    ts: string;
    permalink: string | null;
}

export const slackApi = {
    status: () => backend.call<SlackStatus>("status"),
    signIn: (token: string) => backend.call<SlackStatus>("signIn", { token }),
    signOut: (workspace: string) => backend.call<void>("signOut", { workspace }),
    setDefault: (id: string) => backend.call<void>("setDefault", { id }),
    threadOf: (link: string) => backend.call<SlackThread>("thread", { link }),
    reply: (workspace: string, channel: string, threadTs: string, text: string) =>
        backend.call<SlackPosted>("post", { workspace, channel, threadTs, text }),
};
