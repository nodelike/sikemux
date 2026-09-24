import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { IconAws } from "../../plugin-api/ui";
import { AwsOverlay } from "./components/AwsOverlay";
import { AwsTopBarItem } from "./components/AwsTopBarItem";
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
    openShortcut: "Alt+KeyA",
    Overlay: AwsOverlay,
    TopBarItem: AwsTopBarItem,
});
