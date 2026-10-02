import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { IconAws, IS_MACOS } from "../../plugin-api/ui";
import { AWS_CONSOLE, AWS_PLUGIN_ID } from "./kinds";
import { openAwsSession } from "./state";

const AwsPane = lazy(() => import("./components/AwsPane").then((module) => ({ default: module.AwsPane })));

registerFrontendPlugin({
    id: AWS_PLUGIN_ID,
    surfaces: [
        {
            kind: AWS_CONSOLE,
            title: "AWS",
            icon: (size) => <IconAws size={size} className="icon-aws" />,
            render: ({ visible }) => <AwsPane active={visible} />,
        },
    ],
    open: openAwsSession,
    openTitle: "Open AWS",
    openShortcut: IS_MACOS ? "Meta+Alt+KeyA" : "Ctrl+Alt+KeyA",
});
