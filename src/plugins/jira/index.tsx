import { lazy } from "react";
import { registerFrontendPlugin, signInOf } from "../../plugin-api";
import { useResource } from "../../plugin-api/resources";
import { jiraStatusR } from "./resources";
import { JiraMark } from "./components/JiraMark";
import { JIRA_ISSUES, JIRA_PLUGIN_ID } from "./kinds";
import { openJira } from "./state";

const JiraPane = lazy(() => import("./components/JiraPane").then((module) => ({ default: module.JiraPane })));

registerFrontendPlugin({
    id: JIRA_PLUGIN_ID,
    surfaces: [
        {
            kind: JIRA_ISSUES,
            title: "Jira",
            icon: (size) => <JiraMark size={size} />,
            render: ({ paneId, visible }) => <JiraPane paneId={paneId} active={visible} />,
        },
    ],
    open: openJira,
    openTitle: "Open Jira",
    mark: (size) => <JiraMark size={size} />,
    linkHosts: [".atlassian.net"],
    useSignIn: function useServiceSignIn() {
        return signInOf(useResource(jiraStatusR));
    },
});
