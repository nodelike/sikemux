import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { gitlabCapabilities, gitlabHostApi } from "./api";
import { GitlabMark } from "./components/GitlabMark";
import { GitlabSignIn } from "./components/GitlabSignIn";
import { GITLAB_PLUGIN } from "./kinds";

registerCodeHost({
    ...GITLAB_PLUGIN,
    ciName: "Pipelines",
    icon: (size) => <GitlabMark size={size} />,
    capabilities: gitlabCapabilities,
    api: gitlabHostApi,
    SignIn: GitlabSignIn,
});

registerFrontendPlugin({
    id: GITLAB_PLUGIN.id,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: `Open ${GITLAB_PLUGIN.name}`,
    mark: (size) => <GitlabMark size={size} />,
    linkHosts: ["gitlab.com"],
    TopBarItem: hostCiGlyph(GITLAB_PLUGIN.id),
});
