import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { DockerMark } from "./components/DockerMark";
import { DOCKER_ENGINE, DOCKER_PLUGIN } from "./kinds";
import { openDocker } from "./state";

const DockerPane = lazy(() => import("./components/DockerPane").then((module) => ({ default: module.DockerPane })));

registerFrontendPlugin({
    id: DOCKER_PLUGIN.id,
    surfaces: [
        {
            kind: DOCKER_ENGINE,
            title: "Docker",
            icon: (size) => <DockerMark size={size} className="icon-docker" />,
            render: ({ paneId, visible }) => <DockerPane paneId={paneId} active={visible} />,
        },
    ],
    open: openDocker,
    openTitle: "Open Docker",
    mark: (size) => <DockerMark size={size} className="icon-docker" />,
});
