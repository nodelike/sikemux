import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { bitbucketHostApi } from "./api";
import { BitbucketMark } from "./components/BitbucketMark";
import { BitbucketSignIn } from "./components/BitbucketSignIn";
import { BITBUCKET_PLUGIN_ID } from "./kinds";

registerCodeHost({
    id: BITBUCKET_PLUGIN_ID,
    name: "Bitbucket",
    ciName: "Pipelines",
    icon: (size) => <BitbucketMark size={size} className="icon-bitbucket" />,
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
            rerunFailed: false,
            rerunJob: false,
            debugLogs: false,
            deleteRuns: false,
        },
        pulls: { draft: true, mergeMethods: ["squash", "merge"], requestChanges: true, reopen: false, mergeability: false },
        issues: false,
        releases: false,
        inbox: false,
    },
    api: bitbucketHostApi,
    SignIn: BitbucketSignIn,
});

registerFrontendPlugin({
    id: BITBUCKET_PLUGIN_ID,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: "Open Bitbucket",
    TopBarItem: hostCiGlyph(BITBUCKET_PLUGIN_ID),
});
