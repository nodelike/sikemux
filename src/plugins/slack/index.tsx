import { lazy } from "react";
import { registerFrontendPlugin, signInOf } from "../../plugin-api";
import { useResource } from "../../plugin-api/resources";
import { slackStatusR } from "./resources";
import { SlackMark } from "./components/SlackMark";
import { SLACK_MESSAGES, SLACK_PLUGIN_ID } from "./kinds";
import { openSlack } from "./state";

const SlackPane = lazy(() => import("./components/SlackPane").then((module) => ({ default: module.SlackPane })));

registerFrontendPlugin({
    id: SLACK_PLUGIN_ID,
    surfaces: [
        {
            kind: SLACK_MESSAGES,
            title: "Slack",
            icon: (size) => <SlackMark size={size} />,
            render: ({ paneId, visible }) => <SlackPane paneId={paneId} active={visible} />,
        },
    ],
    open: openSlack,
    openTitle: "Open Slack",
    mark: (size) => <SlackMark size={size} />,
    linkHosts: [".slack.com"],
    useSignIn: function useServiceSignIn() {
        return signInOf(useResource(slackStatusR));
    },
});
