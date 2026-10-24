import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { VERCEL_PROJECTS } from "./kinds";

export type VercelSection = "overview" | "deployments" | "env" | "domains";
export type EnvironmentFilter = "all" | "production" | "preview";
export type StatusFilter = "all" | "READY" | "ERROR" | "BUILDING" | "CANCELED";

export interface VercelView {
    /** The project open, by name; none shows every project. */
    project: string | null;
    section: VercelSection;
    /** The deployment open, by id. */
    deployment: string | null;
    environment: EnvironmentFilter;
    status: StatusFilter;
    /** Words in the sidebar's Find box. */
    find: string;
    /** The sidebar's width as the person dragged it; none for the default. */
    sideWidth: number | null;
}

const views = definePaneViews<VercelView>({
    project: null,
    section: "overview",
    deployment: null,
    environment: "all",
    status: "all",
    find: "",
    sideWidth: null,
});

export const useVercelView = views.use;
export const updateVercelView = views.update;

export const openVercel = (): string | null => openSurface(VERCEL_PROJECTS);

/** Still on its way: building, queued or starting. */
export const isUnderway = (state: string) => state === "BUILDING" || state === "QUEUED" || state === "INITIALIZING";
