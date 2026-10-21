import type { ReactNode } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import type { Product } from "../state";
import {
    billingR,
    bucketsR,
    buildsR,
    clustersR,
    errorGroupsR,
    functionsR,
    gcpProjectsR,
    instancesR,
    runServicesR,
    sqlInstancesR,
} from "../resources";
import type { Target } from "../api";
import { ProductIcon } from "./icons";
import { RecentErrors } from "./RecentErrors";
import { ago, Button, consoleUrl, Facts, keyOf, PageHeader, Status, visit } from "./parts";
import { Problem } from "./Problem";

function Card({
    product,
    title,
    value,
    detail,
    onOpen,
}: {
    product: Product;
    title: string;
    value: ReactNode;
    detail?: ReactNode;
    onOpen: () => void;
}) {
    return (
        <button type="button" className="gcp-card" onClick={onOpen}>
            <span className="gcp-card-head">
                <ProductIcon product={product} />
                {title}
            </span>
            <span className="gcp-card-value">{value}</span>
            {detail && <span className="gcp-card-detail">{detail}</span>}
        </button>
    );
}

/** The project's front page: how many of each resource there are, recent errors and recent builds. */
export function Overview({ active, target, onOpen }: { active: boolean; target: Target; onOpen: (product: Product, open?: string) => void }) {
    const key = keyOf(target);
    const run = useResourceEnabled(active, runServicesR, ...key);
    const functions = useResourceEnabled(active, functionsR, ...key);
    const instances = useResourceEnabled(active, instancesR, ...key);
    const clusters = useResourceEnabled(active, clustersR, ...key);
    const buckets = useResourceEnabled(active, bucketsR, ...key);
    const sql = useResourceEnabled(active, sqlInstancesR, ...key);
    const builds = useResourceEnabled(active, buildsR, ...key);
    const errors = useResourceEnabled(active, errorGroupsR, ...key, "1d");
    const billing = useResourceEnabled(active, billingR, ...key);
    const project = useResourceEnabled(active, gcpProjectsR, target.account ?? "").data?.find((each) => each.id === target.project);
    const openErrors = (errors.data ?? []).filter((group) => group.status === "OPEN").length;
    const count = (data: unknown[] | undefined, failed: boolean) => (data ? data.length : failed ? "—" : "…");
    const note = (handle: { data: unknown; failure: string | undefined }, detail: string) =>
        !handle.data && handle.failure === "service-disabled" ? "API not enabled" : detail;
    const failing = (run.data ?? []).filter((service) => service.state === "FAILED").length;
    const running = (instances.data ?? []).filter((vm) => vm.status === "RUNNING").length;

    return (
        <div className="gcp-page">
            <PageHeader title="Dashboard">
                <Button onClick={() => visit(consoleUrl("home/dashboard", target.project))}>Open in Cloud console</Button>
            </PageHeader>
            <div className="gcp-two">
                <section className="gcp-box-card" aria-label="Project info">
                    <h2 className="gcp-section-title">Project info</h2>
                    <Facts
                        items={[
                            ["Project name", project?.name ?? target.project],
                            ["Project ID", target.project],
                            ["Project number", project?.number],
                        ]}
                    />
                </section>
                <section className="gcp-box-card" aria-label="Billing">
                    <h2 className="gcp-section-title">Billing</h2>
                    {billing.status === "error" && !billing.data ? (
                        <Problem compact error={billing.error} what="billing" onRetry={() => void billing.refresh()} />
                    ) : !billing.data ? (
                        <p className="gcp-note">Loading…</p>
                    ) : (
                        <>
                            <Facts
                                items={[
                                    ["Billing", billing.data.enabled ? "Enabled" : "Disabled"],
                                    [
                                        "Account",
                                        billing.data.accountName ? `${billing.data.accountName} (${billing.data.account})` : billing.data.account,
                                    ],
                                ]}
                            />
                            {billing.data.budgets && billing.data.budgets.length > 0 ? (
                                <div className="gcp-list">
                                    {billing.data.budgets.map((each) => (
                                        <div key={each.name} className="gcp-list-row">
                                            <span className="gcp-list-main">{each.name}</span>
                                            <span>
                                                {each.lastPeriod
                                                    ? "Last month's spend"
                                                    : each.amount !== null
                                                      ? `${each.amount} ${each.currency ?? ""}`
                                                      : "—"}
                                            </span>
                                            <span className="gcp-muted">{each.thresholds.map((threshold) => `${threshold}%`).join(" · ")}</span>
                                        </div>
                                    ))}
                                </div>
                            ) : (
                                <p className="gcp-note">
                                    {billing.data.budgets
                                        ? "No budgets on this billing account."
                                        : "Budgets need the Billing Viewer role on the account."}
                                </p>
                            )}
                            <Button onClick={() => visit(consoleUrl("billing/linkedaccount", target.project))}>Go to billing</Button>
                        </>
                    )}
                </section>
            </div>
            <section className="gcp-section">
                <h2 className="gcp-section-title">Resources</h2>
                <div className="gcp-cards">
                    <Card
                        product="run"
                        title="Cloud Run"
                        value={count(run.data, run.status === "error")}
                        detail={note(run, failing > 0 ? `${failing} failing` : "services")}
                        onOpen={() => onOpen("run")}
                    />
                    <Card
                        product="functions"
                        title="Cloud Functions"
                        value={count(functions.data, functions.status === "error")}
                        detail={note(functions, "functions")}
                        onOpen={() => onOpen("functions")}
                    />
                    <Card
                        product="compute"
                        title="Compute Engine"
                        value={count(instances.data, instances.status === "error")}
                        detail={note(instances, instances.data ? `${running} running` : "VMs")}
                        onOpen={() => onOpen("compute")}
                    />
                    <Card
                        product="gke"
                        title="Kubernetes Engine"
                        value={count(clusters.data, clusters.status === "error")}
                        detail={note(clusters, "clusters")}
                        onOpen={() => onOpen("gke")}
                    />
                    <Card
                        product="storage"
                        title="Cloud Storage"
                        value={count(buckets.data, buckets.status === "error")}
                        detail={note(buckets, "buckets")}
                        onOpen={() => onOpen("storage")}
                    />
                    <Card
                        product="sql"
                        title="SQL"
                        value={count(sql.data, sql.status === "error")}
                        detail={note(sql, "instances")}
                        onOpen={() => onOpen("sql")}
                    />
                    <Card
                        product="errors"
                        title="Error Reporting"
                        value={errors.data ? openErrors : errors.status === "error" ? "—" : "…"}
                        detail={note(errors, "open in the last day")}
                        onOpen={() => onOpen("errors")}
                    />
                </div>
            </section>
            <div className="gcp-two">
                <section className="gcp-section">
                    <h2 className="gcp-section-title">Errors in the last hour</h2>
                    <RecentErrors active={active} target={target} onOpenLogs={() => onOpen("logging")} />
                </section>
                <section className="gcp-section">
                    <h2 className="gcp-section-title">Recent builds</h2>
                    {builds.data && builds.data.length > 0 ? (
                        <div className="gcp-list">
                            {builds.data.slice(0, 6).map((build) => (
                                <button
                                    key={build.id}
                                    type="button"
                                    className="gcp-list-row"
                                    onClick={() => onOpen("build", `${build.region ?? ""}/${build.id}`)}>
                                    <Status state={build.status} />
                                    <span className="gcp-list-main">{build.trigger || build.repo || build.id.slice(0, 8)}</span>
                                    <span className="gcp-muted">{build.branch}</span>
                                    <span className="gcp-muted">{ago(build.createdAt)}</span>
                                </button>
                            ))}
                        </div>
                    ) : (
                        <p className="gcp-note">{builds.status === "error" ? builds.error : builds.data ? "No builds yet." : "Loading…"}</p>
                    )}
                </section>
            </div>
        </div>
    );
}
