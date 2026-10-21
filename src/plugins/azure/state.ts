import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { AZURE_PORTAL } from "./kinds";

export type AzureSection = "resources" | "compute" | "web" | "storage" | "activity";

/** Every resource group at once. */
export const ALL_GROUPS = "*";

export interface AzureView {
    /** The subscription shown, by id; none for the one the CLI is set to. */
    subscription: string | null;
    /** The resource group shown; ALL_GROUPS for every one. */
    group: string;
    section: AzureSection;
    /** The virtual machine or app open, by name. */
    open: { group: string; name: string } | null;
    find: string;
    /** How far back the activity log looks, in hours. */
    hours: number;
    failedOnly: boolean;
    sideWidth: number | null;
}

const views = definePaneViews<AzureView>({
    subscription: null,
    group: ALL_GROUPS,
    section: "resources",
    open: null,
    find: "",
    hours: 24,
    failedOnly: false,
    sideWidth: null,
});

export const useAzureView = views.use;
export const updateAzureView = views.update;

export const openAzure = (): string | null => openSurface(AZURE_PORTAL);
