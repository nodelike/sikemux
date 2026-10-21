import { registerFrontendPlugin } from "../../plugin-api";
import { hostCiGlyph, registerCodeHost } from "../../plugin-api/codehost";
import { openGitArea } from "../../plugin-api/host";
import { bitbucketCapabilities, bitbucketHostApi } from "./api";
import { BitbucketMark } from "./components/BitbucketMark";
import { BitbucketSignIn } from "./components/BitbucketSignIn";
import { BITBUCKET_PLUGIN } from "./kinds";

registerCodeHost({
    ...BITBUCKET_PLUGIN,
    ciName: "Pipelines",
    icon: (size) => <BitbucketMark size={size} />,
    capabilities: bitbucketCapabilities,
    api: bitbucketHostApi,
    SignIn: BitbucketSignIn,
});

registerFrontendPlugin({
    id: BITBUCKET_PLUGIN.id,
    surfaces: [],
    open: () => void openGitArea("pulls"),
    openTitle: `Open ${BITBUCKET_PLUGIN.name}`,
    mark: (size) => <BitbucketMark size={size} />,
    linkHosts: ["bitbucket.org"],
    TopBarItem: hostCiGlyph(BITBUCKET_PLUGIN.id),
});
