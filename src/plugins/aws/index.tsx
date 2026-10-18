import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { useResourceEnabled } from "../../plugin-api/resources";
import { deriveAuthState } from "./auth";
import { awsIdentityR } from "./resources";
import { IconAws, IS_MACOS } from "../../plugin-api/ui";
import { AWS_CONSOLE, AWS_PLUGIN_ID } from "./kinds";
import { awsSettings, openAwsSession } from "./state";

const AwsPane = lazy(() => import("./components/AwsPane").then((module) => ({ default: module.AwsPane })));

registerFrontendPlugin({
    id: AWS_PLUGIN_ID,
    surfaces: [
        {
            kind: AWS_CONSOLE,
            title: "AWS",
            icon: (size) => <IconAws size={size} />,
            render: ({ visible }) => <AwsPane active={visible} />,
        },
    ],
    open: openAwsSession,
    openTitle: "Open AWS",
    mark: (size) => <IconAws size={size} />,
    openShortcut: IS_MACOS ? "Meta+Alt+KeyA" : "Ctrl+Alt+KeyA",
    useSignIn: function useAwsSignIn() {
        const profile = awsSettings.useSelect((s) => s.profile);
        const auth = deriveAuthState(profile, useResourceEnabled(!!profile, awsIdentityR, profile ?? "", false));
        if (auth.kind === "checking") return "checking";
        if (auth.kind === "authed") return "signed-in";
        return auth.kind === "expired" || auth.kind === "error" ? "failed" : "signed-out";
    },
});
