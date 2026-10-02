import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { avatarForEmail, avatarForLogin, githubHostApi } from "./api";
import { GithubSignIn } from "./components/ActionsSignIn";
import { GithubMark } from "./components/GithubMark";
import { GITHUB_PLUGIN_ID } from "./kinds";

registerCodeHost({
    id: GITHUB_PLUGIN_ID,
    name: "GitHub",
    ciName: "Actions",
    icon: (size) => <GithubMark size={size} className="icon-github" />,
    capabilities: {
        ci: {
            graph: true,
            attempts: true,
            approvals: true,
            dispatch: true,
            annotations: true,
            summaries: true,
            artifacts: true,
            billing: true,
            workflowFile: true,
            rerunFailed: true,
            rerunJob: true,
            debugLogs: true,
            deleteRuns: true,
        },
        pulls: { draft: true, mergeMethods: ["squash", "merge", "rebase"], requestChanges: true, reopen: true, mergeability: true },
        issues: true,
        releases: true,
        inbox: true,
    },
    api: githubHostApi,
    pullHeadRef: (number) => `pull/${number}/head`,
    avatarForEmail,
    avatarForLogin,
    SignIn: GithubSignIn,
});

registerFrontendPlugin({
    id: GITHUB_PLUGIN_ID,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: "Open GitHub",
    TopBarItem: hostCiGlyph(GITHUB_PLUGIN_ID),
});
