import { defineService } from "../../plugin-api/service";
import { slackApi } from "./api";
import { SLACK_PLUGIN } from "./kinds";

export const slack = defineService(SLACK_PLUGIN, slackApi.status);

export const slackChannelsR = slack.resource("channels", (workspace: string) => slackApi.channels(workspace), 300_000);

export const slackHistoryR = slack.resource("history", (workspace: string, channel: string) => slackApi.history(workspace, channel), 20_000);

export const slackThreadR = slack.resource(
    "thread",
    (workspace: string, channel: string, ts: string) => slackApi.thread(workspace, channel, ts),
    20_000,
);
