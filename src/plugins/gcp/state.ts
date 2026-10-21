import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { definePluginSettings } from "../../plugin-api/settings";
import { GCP_CONSOLE, GCP_PLUGIN } from "./kinds";

export type Product =
    | "overview"
    | "run"
    | "functions"
    | "compute"
    | "gke"
    | "storage"
    | "sql"
    | "bigquery"
    | "pubsub"
    | "build"
    | "registry"
    | "scheduler"
    | "secrets"
    | "iam"
    | "network"
    | "dns"
    | "logging"
    | "monitoring"
    | "errors";

export const PRODUCTS: { id: Product; label: string; group: string }[] = [
    { id: "overview", label: "Dashboard", group: "" },
    { id: "run", label: "Cloud Run", group: "Serverless" },
    { id: "functions", label: "Cloud Functions", group: "Serverless" },
    { id: "compute", label: "Compute Engine", group: "Compute" },
    { id: "gke", label: "Kubernetes Engine", group: "Compute" },
    { id: "storage", label: "Cloud Storage", group: "Storage" },
    { id: "sql", label: "SQL", group: "Databases" },
    { id: "bigquery", label: "BigQuery", group: "Analytics" },
    { id: "pubsub", label: "Pub/Sub", group: "Analytics" },
    { id: "build", label: "Cloud Build", group: "CI/CD" },
    { id: "registry", label: "Artifact Registry", group: "CI/CD" },
    { id: "scheduler", label: "Cloud Scheduler", group: "Integration" },
    { id: "network", label: "VPC network", group: "Networking" },
    { id: "dns", label: "Cloud DNS", group: "Networking" },
    { id: "secrets", label: "Secret Manager", group: "Security" },
    { id: "iam", label: "IAM", group: "Security" },
    { id: "logging", label: "Logging", group: "Operations" },
    { id: "monitoring", label: "Monitoring", group: "Operations" },
    { id: "errors", label: "Error Reporting", group: "Operations" },
];

export interface GcpSettings {
    /** The gcloud account calls run as; none uses gcloud's active one. */
    account: string | null;
    project: string | null;
    sideWidth: number | null;
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const someText = (value: unknown) => (typeof value === "string" && value ? value : null);

export const gcpSettings = definePluginSettings<GcpSettings>(GCP_PLUGIN.id, (saved) => {
    const raw = isRecord(saved) ? saved : {};
    return {
        account: someText(raw.account),
        project: someText(raw.project),
        sideWidth: typeof raw.sideWidth === "number" && raw.sideWidth >= 160 && raw.sideWidth <= 420 ? raw.sideWidth : null,
    };
});

export const setGcpAccount = (account: string | null) => gcpSettings.update((settings) => ({ ...settings, account, project: null }));
export const setGcpProject = (project: string | null) => gcpSettings.update((settings) => ({ ...settings, project }));
export const setGcpSideWidth = (sideWidth: number | null) => gcpSettings.update((settings) => ({ ...settings, sideWidth }));

/** Where a pane is in the console: the product, the resource open in it, and that resource's tab. */
export interface GcpView {
    product: Product;
    /** The resource open, as the product names it, such as `us-central1/api` for a Cloud Run service. */
    open: string | null;
    tab: string | null;
    /** Cloud Run's services or jobs, Cloud Build's history or triggers, Pub/Sub's topics or subscriptions. */
    list: string | null;
    /** The folder open in a bucket. */
    prefix: string;
    filter: string;
}

const views = definePaneViews<GcpView>({ product: "overview", open: null, tab: null, list: null, prefix: "", filter: "" });

export const useGcpView = views.use;
export const getGcpView = views.get;
export const updateGcpView = views.update;

/** Opens a product, closing whatever was open in the one before. */
export const openProduct = (paneId: string, product: Product) =>
    updateGcpView(paneId, { product, open: null, tab: null, list: null, prefix: "", filter: "" });

export const openGcp = (): string | null => openSurface(GCP_CONSOLE);

/** `us-central1/api` as its location and name. */
export function splitOpen(open: string | null): { location: string; name: string } | null {
    if (!open) return null;
    const slash = open.indexOf("/");
    return slash < 0 ? { location: "", name: open } : { location: open.slice(0, slash), name: open.slice(slash + 1) };
}
