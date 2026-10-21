import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { linear } from "./resources";
import { LinearMark } from "./components/LinearMark";
import { LINEAR_ISSUES, LINEAR_PLUGIN } from "./kinds";
import { openLinear } from "./state";

const LinearPane = lazy(() => import("./components/LinearPane").then((module) => ({ default: module.LinearPane })));

registerFrontendPlugin({
    id: LINEAR_PLUGIN.id,
    surfaces: [
        {
            kind: LINEAR_ISSUES,
            title: LINEAR_PLUGIN.name,
            icon: (size) => <LinearMark size={size} className="icon-linear" />,
            render: ({ paneId, visible }) => <LinearPane paneId={paneId} active={visible} />,
        },
    ],
    open: openLinear,
    openTitle: `Open ${LINEAR_PLUGIN.name}`,
    mark: (size) => <LinearMark size={size} className="icon-linear" />,
    linkHosts: ["linear.app"],
    useSignIn: linear.useSignIn,
});
