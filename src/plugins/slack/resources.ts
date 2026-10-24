import { defineService } from "../../plugin-api/service";
import { slackApi } from "./api";
import { SLACK_PLUGIN } from "./kinds";
import type { SlackChannels, SlackHistory, SlackOpenThread } from "./types.generated";

export const slack = defineService(SLACK_PLUGIN, slackApi.status);

/** A workspace's channels and direct messages, kept by the core for the pane. */
export const slackChannelsV = slack.view<[workspace: string], SlackChannels>("channelsView", (workspace) => ({ workspace }));

/** A channel's latest messages, kept by the core. */
export const slackHistoryV = slack.view<[workspace: string, channel: string], SlackHistory>("historyView", (workspace, channel) => ({
    workspace,
    channel,
}));

/** A thread with what an agent is handed of it, kept by the core. */
export const slackThreadV = slack.view<[workspace: string, channel: string, ts: string], SlackOpenThread>("threadView", (workspace, channel, ts) => ({
    workspace,
    channel,
    ts,
}));
