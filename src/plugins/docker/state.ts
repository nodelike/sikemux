import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { DOCKER_ENGINE } from "./kinds";

export type DockerSection = "containers" | "stacks" | "images" | "volumes" | "disk";
export type ContainerTab = "logs" | "inspect" | "stats";
export type StateFilter = "all" | "running" | "stopped";

export interface DockerView {
    section: DockerSection;
    /** The container open, by name. */
    container: string | null;
    tab: ContainerTab;
    stateFilter: StateFilter;
    /** Words in the sidebar's Find box. */
    find: string;
    /** The sidebar's width as the person dragged it; none for the default. */
    sideWidth: number | null;
}

const views = definePaneViews<DockerView>({
    section: "containers",
    container: null,
    tab: "logs",
    stateFilter: "all",
    find: "",
    sideWidth: null,
});

export const useDockerView = views.use;
export const updateDockerView = views.update;

export const openDocker = (): string | null => openSurface(DOCKER_ENGINE);
