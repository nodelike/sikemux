import { createPluginBackend } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { SLACK_PLUGIN_ID } from "./kinds";

export { failureMessage } from "../../plugin-api/backend";

const backend = createPluginBackend(SLACK_PLUGIN_ID, { signInStatus: (kind) => kind === "slack.status" });

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

export interface SlackChannel {
    id: string;
    name: string;
    kind: "channel" | "private" | "dm" | "group";
    updated: number | null;
}

export interface SlackMessage {
    ts: string;
    user: string | null;
    name: string;
    bot: boolean;
    /** Markdown, people and channels named. */
    text: string;
    at: string;
    threadTs: string | null;
    replyCount: number;
    files: string[];
}

export interface SlackThread {
    workspace: string;
    channel: string;
    ts: string;
    permalink: string | null;
    messages: SlackMessage[];
    truncated: boolean;
}

export interface SlackPosted {
    channel: string;
    ts: string;
    permalink: string | null;
}

export const refreshSlack = () => invalidate((kind) => kind.startsWith("slack."));

export const slackApi = {
    status: () => backend.call<SlackStatus>("status"),
    signIn: (token: string) => backend.call<SlackStatus>("signIn", { token }),
    signOut: (workspace: string) => backend.call<void>("signOut", { workspace }),
    setDefault: (id: string) => backend.call<void>("setDefault", { id }),
    channels: (workspace: string) => backend.call<SlackChannel[]>("channels", { workspace }),
    history: (workspace: string, channel: string) => backend.call<SlackMessage[]>("history", { workspace, channel, limit: 50 }),
    thread: (workspace: string, channel: string, ts: string) => backend.call<SlackThread>("thread", { workspace, channel, ts }),
    threadOf: (link: string) => backend.call<SlackThread>("thread", { link }),
    reply: (workspace: string, channel: string, threadTs: string, text: string) =>
        backend.call<SlackPosted>("post", { workspace, channel, threadTs, text }),
};
