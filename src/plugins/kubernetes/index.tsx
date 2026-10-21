import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { KubernetesMark } from "./components/KubernetesMark";
import { KUBERNETES_CLUSTER, KUBERNETES_PLUGIN } from "./kinds";
import { openKubernetes } from "./state";

const KubernetesPane = lazy(() => import("./components/KubernetesPane").then((module) => ({ default: module.KubernetesPane })));

registerFrontendPlugin({
    id: KUBERNETES_PLUGIN.id,
    surfaces: [
        {
            kind: KUBERNETES_CLUSTER,
            title: "Kubernetes",
            icon: (size) => <KubernetesMark size={size} className="icon-kubernetes" />,
            render: ({ paneId, visible }) => <KubernetesPane paneId={paneId} active={visible} />,
        },
    ],
    open: openKubernetes,
    openTitle: "Open Kubernetes",
    mark: (size) => <KubernetesMark size={size} className="icon-kubernetes" />,
});
