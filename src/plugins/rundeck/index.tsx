import { lazy } from "react";
import { registerFrontendPlugin, signInOf } from "../../plugin-api";
import { useResource } from "../../plugin-api/resources";
import { rndStatusR } from "./resources";
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
            icon: (size) => <IconRundeck size={size} />,
            render: ({ paneId, visible }) => <RundeckPane paneId={paneId} active={visible} />,
            quickOpen: toggleRundeckJobPalette,
        },
    ],
    open: openRundeckSession,
    openTitle: "Open Rundeck deploy center",
    mark: (size) => <IconRundeck size={size} />,
    Overlay: RundeckOverlay,
    TopBarItem: RundeckTopBarItem,
    useSignIn: function useRundeckSignIn() {
        const status = useResource(rndStatusR);
        return signInOf({ ...status, data: status.data && { configured: status.data.configured, authFailed: status.data.auth_failed } });
    },
});
