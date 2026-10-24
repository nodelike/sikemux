import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { LINEAR_ISSUES } from "./kinds";

export interface LinearView {
    /** The workspace shown; empty for the default one. */
    workspace: string;
    /** Words searched for; empty lists the person's open issues. */
    query: string;
    /** The issue open beside the list, by identifier. */
    issue: string | null;
}

const views = definePaneViews<LinearView>({ workspace: "", query: "", issue: null });

export const useLinearView = views.use;
export const updateLinearView = views.update;

export const openLinear = (): string | null => openSurface(LINEAR_ISSUES);
