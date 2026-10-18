import { lazy } from "react";
import { registerFrontendPlugin, signInOf } from "../../plugin-api";
import { useResource } from "../../plugin-api/resources";
import { vercelStatusR } from "./resources";
import { VercelMark } from "./components/VercelMark";
import { VERCEL_PLUGIN_ID, VERCEL_PROJECTS } from "./kinds";
import { openVercel } from "./state";

const VercelPane = lazy(() => import("./components/VercelPane").then((module) => ({ default: module.VercelPane })));

registerFrontendPlugin({
    id: VERCEL_PLUGIN_ID,
    surfaces: [
        {
            kind: VERCEL_PROJECTS,
            title: "Vercel",
            icon: (size) => <VercelMark size={size} className="icon-vercel" />,
            render: ({ paneId, visible }) => <VercelPane paneId={paneId} active={visible} />,
        },
    ],
    open: openVercel,
    openTitle: "Open Vercel",
    mark: (size) => <VercelMark size={size} className="icon-vercel" />,
    linkHosts: ["vercel.com"],
    useSignIn: function useServiceSignIn() {
        return signInOf(useResource(vercelStatusR));
    },
});
