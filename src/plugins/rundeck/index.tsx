import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { IconRundeck } from "../../plugin-api/ui";
import { RundeckOverlay } from "./components/RundeckOverlay";
import { RundeckTopBarItem } from "./components/RundeckTopBarItem";
import { RUNDECK_DEPLOY, RUNDECK_PLUGIN_ID } from "./kinds";
import { openRundeckSession, toggleRundeckJobPalette } from "./state";

const RundeckPane = lazy(() => import("./components/RundeckPane").then((module) => ({ default: module.RundeckPane })));

registerFrontendPlugin({
    id: RUNDECK_PLUGIN_ID,
    surfaces: [
        {
            kind: RUNDECK_DEPLOY,
            title: "Rundeck",
            icon: (size) => <IconRundeck size={Math.round(size * 0.87)} />,
            render: ({ paneId, visible }) => <RundeckPane paneId={paneId} active={visible} />,
            quickOpen: toggleRundeckJobPalette,
        },
    ],
    open: openRundeckSession,
    openTitle: "Open Rundeck deploy center",
    Overlay: RundeckOverlay,
    TopBarItem: RundeckTopBarItem,
});
