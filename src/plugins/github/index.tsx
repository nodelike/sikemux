import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { avatarForEmail, avatarForLogin, githubCapabilities, githubHostApi } from "./api";
import { GithubSignIn } from "./components/ActionsSignIn";
import { GithubMark } from "./components/GithubMark";
import { GITHUB_PLUGIN } from "./kinds";

registerCodeHost({
    ...GITHUB_PLUGIN,
    ciName: "Actions",
    icon: (size) => <GithubMark size={size} className="icon-github" />,
    capabilities: githubCapabilities,
    api: githubHostApi,
    avatarForEmail,
    avatarForLogin,
    SignIn: GithubSignIn,
});

registerFrontendPlugin({
    id: GITHUB_PLUGIN.id,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: `Open ${GITHUB_PLUGIN.name}`,
    mark: (size) => <GithubMark size={size} className="icon-github" />,
    linkHosts: ["github.com"],
    TopBarItem: hostCiGlyph(GITHUB_PLUGIN.id),
});
