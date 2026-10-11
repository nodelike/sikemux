import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { gitlabHostApi } from "./api";
import { GitlabMark } from "./components/GitlabMark";
import { GitlabSignIn } from "./components/GitlabSignIn";
import { GITLAB_PLUGIN_ID } from "./kinds";

registerCodeHost({
    id: GITLAB_PLUGIN_ID,
    name: "GitLab",
    ciName: "Pipelines",
    icon: (size) => <GitlabMark size={size} className="icon-gitlab" />,
    capabilities: {
        ci: {
            graph: true,
            attempts: false,
            approvals: false,
            dispatch: true,
            annotations: false,
            summaries: false,
            artifacts: false,
            billing: false,
            workflowFile: true,
            rerunFailed: true,
            rerunJob: true,
            debugLogs: false,
            deleteRuns: true,
        },
        pulls: { draft: true, mergeMethods: ["merge", "squash"], requestChanges: false, reopen: true, mergeability: true },
        issues: true,
        releases: true,
        inbox: true,
    },
    api: gitlabHostApi,
    pullHeadRef: (number) => `merge-requests/${number}/head`,
    SignIn: GitlabSignIn,
});

registerFrontendPlugin({
    id: GITLAB_PLUGIN_ID,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: "Open GitLab",
    mark: (size) => <GitlabMark size={size} className="icon-gitlab" />,
    linkHosts: ["gitlab.com"],
    TopBarItem: hostCiGlyph(GITLAB_PLUGIN_ID),
});
