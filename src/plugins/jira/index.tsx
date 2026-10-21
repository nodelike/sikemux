import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { jira } from "./resources";
import { JiraMark } from "./components/JiraMark";
import { JIRA_ISSUES, JIRA_PLUGIN } from "./kinds";
import { openJira } from "./state";

const JiraPane = lazy(() => import("./components/JiraPane").then((module) => ({ default: module.JiraPane })));

registerFrontendPlugin({
    id: JIRA_PLUGIN.id,
    surfaces: [
        {
            kind: JIRA_ISSUES,
            title: JIRA_PLUGIN.name,
            icon: (size) => <JiraMark size={size} />,
            render: ({ paneId, visible }) => <JiraPane paneId={paneId} active={visible} />,
        },
    ],
    open: openJira,
    openTitle: `Open ${JIRA_PLUGIN.name}`,
    mark: (size) => <JiraMark size={size} />,
    linkHosts: [".atlassian.net"],
    useSignIn: jira.useSignIn,
});
