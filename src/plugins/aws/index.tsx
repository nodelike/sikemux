import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { aws } from "./resources";
import { IconAws, IS_MACOS } from "../../plugin-api/ui";
import { AWS_CONSOLE, AWS_PLUGIN } from "./kinds";
import { openAwsSession } from "./state";

const AwsPane = lazy(() => import("./components/AwsPane").then((module) => ({ default: module.AwsPane })));

registerFrontendPlugin({
    id: AWS_PLUGIN.id,
    surfaces: [
        {
            kind: AWS_CONSOLE,
            title: AWS_PLUGIN.name,
            icon: (size) => <IconAws size={size} />,
            render: ({ paneId, visible }) => <AwsPane paneId={paneId} active={visible} />,
        },
    ],
    open: openAwsSession,
    openTitle: `Open ${AWS_PLUGIN.name}`,
    mark: (size) => <IconAws size={size} />,
    openShortcut: IS_MACOS ? "Meta+Alt+KeyA" : "Ctrl+Alt+KeyA",
    useSignIn: aws.useSignIn,
});
