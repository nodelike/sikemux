import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { AzureMark } from "./components/AzureMark";
import { AZURE_PLUGIN, AZURE_PORTAL } from "./kinds";
import { azure } from "./resources";
import { openAzure } from "./state";

const AzurePane = lazy(() => import("./components/AzurePane").then((module) => ({ default: module.AzurePane })));

registerFrontendPlugin({
    id: AZURE_PLUGIN.id,
    surfaces: [
        {
            kind: AZURE_PORTAL,
            title: AZURE_PLUGIN.name,
            icon: (size) => <AzureMark size={size} className="icon-azure" />,
            render: ({ paneId, visible }) => <AzurePane paneId={paneId} active={visible} />,
        },
    ],
    open: openAzure,
    openTitle: `Open ${AZURE_PLUGIN.name}`,
    mark: (size) => <AzureMark size={size} className="icon-azure" />,
    linkHosts: ["portal.azure.com"],
    useSignIn: azure.useSignIn,
});
