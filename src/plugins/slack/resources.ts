import { resource } from "../../plugin-api/resources";
import { slackApi, type SlackChannel, type SlackMessage, type SlackStatus, type SlackThread } from "./api";

export const slackStatusR = resource({
    kind: "slack.status",
    fetch: (): Promise<SlackStatus> => slackApi.status(),
    staleAfterMs: 60_000,
});

export const slackChannelsR = resource({
    kind: "slack.channels",
    fetch: (workspace: string): Promise<SlackChannel[]> => slackApi.channels(workspace),
    staleAfterMs: 300_000,
});

export const slackHistoryR = resource({
    kind: "slack.history",
    fetch: (workspace: string, channel: string): Promise<SlackMessage[]> => slackApi.history(workspace, channel),
    staleAfterMs: 20_000,
});

export const slackThreadR = resource({
    kind: "slack.thread",
    fetch: (workspace: string, channel: string, ts: string): Promise<SlackThread> => slackApi.thread(workspace, channel, ts),
    staleAfterMs: 20_000,
});
