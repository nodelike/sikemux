import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { signoz } from "./resources";
import { SignozIcon } from "./components/SignozIcon";
import { SignozOverlay } from "./components/SignozOverlay";
import { SIGNOZ_EXPLORE, SIGNOZ_PLUGIN } from "./kinds";
import { openSignoz, togglePalette } from "./state";

const SignozPane = lazy(() => import("./components/SignozPane").then((module) => ({ default: module.SignozPane })));

registerFrontendPlugin({
    id: SIGNOZ_PLUGIN.id,
    surfaces: [
        {
            kind: SIGNOZ_EXPLORE,
            title: SIGNOZ_PLUGIN.name,
            icon: (size) => <SignozIcon size={size} />,
            render: ({ paneId, visible }) => <SignozPane paneId={paneId} active={visible} />,
            quickOpen: togglePalette,
        },
    ],
    open: openSignoz,
    openTitle: `Open ${SIGNOZ_PLUGIN.name}`,
    mark: (size) => <SignozIcon size={size} />,
    Overlay: SignozOverlay,
    useSignIn: signoz.useSignIn,
});
