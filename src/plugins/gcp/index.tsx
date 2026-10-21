import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { signInOf } from "../../plugin-api/signInState";
import { useResource } from "../../plugin-api/resources";
import { gcpStatusR } from "./resources";
import { GcpMark } from "./components/icons";
import { GCP_CONSOLE, GCP_PLUGIN } from "./kinds";
import { openGcp } from "./state";

const GcpPane = lazy(() => import("./components/GcpPane").then((module) => ({ default: module.GcpPane })));

registerFrontendPlugin({
    id: GCP_PLUGIN.id,
    surfaces: [
        {
            kind: GCP_CONSOLE,
            title: GCP_PLUGIN.name,
            icon: (size) => <GcpMark size={size} />,
            render: ({ paneId, visible }) => <GcpPane paneId={paneId} active={visible} />,
        },
    ],
    open: openGcp,
    openTitle: `Open ${GCP_PLUGIN.name}`,
    mark: (size) => <GcpMark size={size} />,
    linkHosts: ["console.cloud.google.com"],
    useSignIn: function useServiceSignIn() {
        const status = useResource(gcpStatusR);
        return signInOf({
            ...status,
            data: status.data && { configured: status.data.installed && status.data.accounts.length > 0, authFailed: false },
        });
    },
});
