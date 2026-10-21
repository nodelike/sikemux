import { useEffect, useRef, type ComponentType } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Dropdown, ResizeHandle, SkeletonRows } from "../../../plugin-api/ui";
import { type GcpStatus, type Target } from "../api";
import { gcpStatusR, refreshGcp } from "../resources";
import {
    gcpSettings,
    getGcpView,
    openProduct,
    PRODUCTS,
    setGcpAccount,
    setGcpProject,
    setGcpSideWidth,
    updateGcpView,
    useGcpView,
    type Product,
} from "../state";
import { AgentSetup } from "./AgentSetup";
import { GcpMark, ProductIcon } from "./icons";
import { LoggingPage } from "./LoggingPage";
import { RunPage } from "./RunPage";
import { ComputePage } from "./ComputePage";
import { GkePage } from "./GkePage";
import { StoragePage } from "./StoragePage";
import { SqlPage } from "./SqlPage";
import { PubSubPage } from "./PubSubPage";
import { BuildPage } from "./BuildPage";
import { SecretsPage } from "./SecretsPage";
import { FunctionsPage } from "./FunctionsPage";
import { ErrorsPage } from "./ErrorsPage";
import { BigQueryPage } from "./BigQueryPage";
import { RegistryPage } from "./RegistryPage";
import { SchedulerPage } from "./SchedulerPage";
import { IamPage } from "./IamPage";
import { NetworkPage } from "./NetworkPage";
import { DnsPage } from "./DnsPage";
import { MonitoringPage } from "./MonitoringPage";
import { Overview } from "./Overview";
import { ProjectList, ProjectPicker } from "./ProjectPicker";
import { Search } from "./Search";
import { Setup } from "./Setup";
import { useTarget } from "./parts";
import "../gcp.css";

export interface ProductProps {
    paneId: string;
    active: boolean;
    target: Target;
}

/** Each product's page; a product without one yet is left out of the sidebar. */
const PAGES: Partial<Record<Product, ComponentType<ProductProps>>> = {
    run: RunPage,
    functions: FunctionsPage,
    compute: ComputePage,
    gke: GkePage,
    storage: StoragePage,
    sql: SqlPage,
    bigquery: BigQueryPage,
    pubsub: PubSubPage,
    build: BuildPage,
    registry: RegistryPage,
    scheduler: SchedulerPage,
    secrets: SecretsPage,
    iam: IamPage,
    network: NetworkPage,
    dns: DnsPage,
    logging: LoggingPage,
    errors: ErrorsPage,
    monitoring: MonitoringPage,
};

function typing(target: EventTarget): boolean {
    return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** Esc climbs one level: out of a resource to its list, out of a product to the dashboard. */
export function goBack(paneId: string): boolean {
    const view = getGcpView(paneId);
    if (view.open || view.prefix) {
        updateGcpView(paneId, view.prefix && view.product === "storage" ? { prefix: "" } : { open: null, tab: null });
        return true;
    }
    if (view.product !== "overview") {
        openProduct(paneId, "overview");
        return true;
    }
    return false;
}

const SIDE_MIN = 160;
const SIDE_MAX = 420;

export function GcpPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, gcpStatusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Asking gcloud who is signed in" />;
    if (!status.data?.installed || status.data.accounts.length === 0) return <Setup status={status.data} />;
    return <Console paneId={paneId} active={active} status={status.data} />;
}

/** The project gcloud's active configuration names, so a first visit opens on it. */
function useDefaultProject(status: GcpStatus) {
    const project = gcpSettings.useSelect((s) => s.project);
    const account = gcpSettings.useSelect((s) => s.account);
    useEffect(() => {
        if (project) return;
        const configuration = status.configurations.find((each) => each.active);
        const sameAccount = !account || configuration?.account === account;
        if (configuration?.project && sameAccount) setGcpProject(configuration.project);
    }, [project, account, status]);
}

function Console({ paneId, active, status }: { paneId: string; active: boolean; status: GcpStatus }) {
    const view = useGcpView(paneId);
    const target = useTarget();
    const sideRef = useRef<HTMLDivElement>(null);
    const sideWidth = gcpSettings.useSelect((s) => s.sideWidth);
    const account = gcpSettings.useSelect((s) => s.account);
    useDefaultProject(status);
    const activeAccount = status.accounts.find((each) => each.active)?.account;
    const Page = PAGES[view.product];
    const open = (product: Product, resource?: string) => {
        openProduct(paneId, product);
        if (resource) updateGcpView(paneId, { open: resource });
    };

    return (
        <div
            className="gcp-pane"
            onKeyDown={(event) => {
                if (event.key !== "Escape" || event.defaultPrevented || typing(event.target)) return;
                if (goBack(paneId)) event.preventDefault();
            }}>
            <header className="gcp-topbar">
                <span className="gcp-brand">
                    <GcpMark size={22} />
                    <span>Google Cloud</span>
                </span>
                <ProjectPicker active={active} />
                <Search
                    active={active}
                    target={target}
                    onOpen={(hit) => {
                        openProduct(paneId, hit.product);
                        if (hit.open || hit.list) updateGcpView(paneId, { open: hit.open, list: hit.list ?? null });
                    }}
                />
                <span className="gcp-topbar-gap" />
                <Dropdown
                    className="gcp-account"
                    label="Account"
                    value={account ?? ""}
                    options={[
                        { value: "", label: activeAccount ? `${activeAccount} (gcloud's active)` : "gcloud's active account" },
                        ...status.accounts
                            .filter((each) => each.account !== activeAccount)
                            .map((each) => ({
                                value: each.account,
                                label: each.account,
                                detail: each.serviceAccount ? "Service account" : undefined,
                            })),
                    ]}
                    onChange={(value) => setGcpAccount(value || null)}
                />
                <button type="button" className="gcp-icon-button" aria-label="Refresh" title="Refresh" onClick={() => refreshGcp()}>
                    <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true">
                        <path d="M17.6 6.4A8 8 0 1 0 19.7 14h-2.1a6 6 0 1 1-1.4-6.2L13 11h7V4z" fill="currentColor" />
                    </svg>
                </button>
            </header>
            <div className="gcp-body">
                <aside className="gcp-nav" ref={sideRef} style={sideWidth ? { width: sideWidth } : undefined} aria-label="Products">
                    {PRODUCTS.filter((product) => product.id === "overview" || PAGES[product.id]).map((product, index, shown) => {
                        const group = product.group && shown[index - 1]?.group !== product.group ? product.group : null;
                        return (
                            <div key={product.id}>
                                {group && <div className="gcp-nav-group">{group}</div>}
                                <button
                                    type="button"
                                    className={`gcp-nav-item${view.product === product.id ? " on" : ""}`}
                                    aria-current={view.product === product.id ? "page" : undefined}
                                    disabled={!target}
                                    onClick={() => openProduct(paneId, product.id)}>
                                    <ProductIcon product={product.id} />
                                    <span>{product.label}</span>
                                </button>
                            </div>
                        );
                    })}
                    <AgentSetup />
                </aside>
                <ResizeHandle
                    targetRef={sideRef}
                    axis="x"
                    grows={1}
                    min={SIDE_MIN}
                    max={() => SIDE_MAX}
                    size={sideWidth}
                    label="Resize the Google Cloud sidebar"
                    className="gcp-resize"
                    onResize={(size) => {
                        if (size === null && sideRef.current) sideRef.current.style.width = "";
                        setGcpSideWidth(size);
                    }}
                />
                <main className="gcp-main">
                    {!target ? (
                        <div className="gcp-page">
                            <h1 className="gcp-page-title">Select a project</h1>
                            <ProjectList active={active} />
                        </div>
                    ) : view.product === "overview" || !Page ? (
                        <Overview active={active} target={target} onOpen={open} />
                    ) : (
                        <Page paneId={paneId} active={active} target={target} />
                    )}
                </main>
            </div>
        </div>
    );
}
