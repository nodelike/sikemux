import { errorText } from "../../../plugin-api/format";
import { refreshGcp } from "../resources";
import { useState } from "react";
import { notify } from "../../../plugin-api/host";
import { isPluginFailure } from "../../../plugin-api/backend";
import { gcpApi } from "../api";
import type { Product } from "../state";
import { ProductIcon } from "./icons";
import { Button, consoleUrl, useTarget, visit } from "./parts";

const APIS: Record<string, { name: string; product: Product }> = {
    "compute.googleapis.com": { name: "Compute Engine API", product: "compute" },
    "run.googleapis.com": { name: "Cloud Run Admin API", product: "run" },
    "cloudfunctions.googleapis.com": { name: "Cloud Functions API", product: "functions" },
    "container.googleapis.com": { name: "Kubernetes Engine API", product: "gke" },
    "storage.googleapis.com": { name: "Cloud Storage API", product: "storage" },
    "storage-api.googleapis.com": { name: "Cloud Storage API", product: "storage" },
    "sqladmin.googleapis.com": { name: "Cloud SQL Admin API", product: "sql" },
    "bigquery.googleapis.com": { name: "BigQuery API", product: "bigquery" },
    "pubsub.googleapis.com": { name: "Cloud Pub/Sub API", product: "pubsub" },
    "cloudbuild.googleapis.com": { name: "Cloud Build API", product: "build" },
    "artifactregistry.googleapis.com": { name: "Artifact Registry API", product: "registry" },
    "cloudscheduler.googleapis.com": { name: "Cloud Scheduler API", product: "scheduler" },
    "dns.googleapis.com": { name: "Cloud DNS API", product: "dns" },
    "secretmanager.googleapis.com": { name: "Secret Manager API", product: "secrets" },
    "iam.googleapis.com": { name: "Identity and Access Management (IAM) API", product: "iam" },
    "cloudresourcemanager.googleapis.com": { name: "Cloud Resource Manager API", product: "iam" },
    "logging.googleapis.com": { name: "Cloud Logging API", product: "logging" },
    "monitoring.googleapis.com": { name: "Cloud Monitoring API", product: "monitoring" },
    "clouderrorreporting.googleapis.com": { name: "Error Reporting API", product: "errors" },
    "cloudbilling.googleapis.com": { name: "Cloud Billing API", product: "overview" },
};

/** A failure's message without the `sikemux.gcp: gcp:` prefixes, starting with a capital. */
export function plainError(error: string | undefined): string {
    const message = (error ?? "").replace(/^(sikemux\.gcp:\s*)?(gcp:\s*)?/, "").trim();
    return message ? message[0].toUpperCase() + message.slice(1) : "Something went wrong.";
}

/** The API a "turned off" failure names, or null for any other failure. */
export function disabledApi(error: string | undefined): string | null {
    return /the (\S+) API is turned off/.exec(error ?? "")?.[1] ?? null;
}

/** What went wrong loading a page, with the step that fixes it: turning an API on, or trying again. */
export function Problem({ error, what, onRetry, compact }: { error: string | undefined; what: string; onRetry: () => void; compact?: boolean }) {
    const service = disabledApi(error);
    if (service) return <ApiOff service={service} compact={compact} />;
    const denied = /not allowed:/.test(error ?? "");
    return (
        <div className={`gcp-problem${compact ? " compact" : ""}`} role="alert">
            {!compact && <h3>{denied ? "You don't have access" : `Couldn't load ${what}`}</h3>}
            <p>{plainError(error).replace(/^Not allowed:\s*/, "")}</p>
            <div className="gcp-problem-actions">
                <Button onClick={onRetry}>Try again</Button>
            </div>
        </div>
    );
}

function ApiOff({ service, compact }: { service: string; compact?: boolean }) {
    const target = useTarget();
    const [enabling, setEnabling] = useState(false);
    const [unbilled, setUnbilled] = useState(false);
    const known = APIS[service];
    const name = known?.name ?? (service === "needed" ? "A required API" : service);
    const project = target?.project ?? "";
    const library = service === "needed" ? consoleUrl("apis/library", project) : consoleUrl(`apis/library/${service}`, project);

    const enable = () => {
        if (!target) return;
        setEnabling(true);
        void gcpApi
            .enableService(target, service)
            .then(() => {
                notify("info", `${name} is on.`);
                refreshGcp();
            })
            .catch((failure: unknown) => {
                if (isPluginFailure(failure, "billing-disabled")) setUnbilled(true);
                else notify("error", `Enable ${name}: ${plainError(errorText(failure))}`);
            })
            .finally(() => setEnabling(false));
    };

    return (
        <div className={`gcp-problem api-off${compact ? " compact" : ""}`}>
            {!compact && (
                <div className="gcp-problem-icon">
                    <ProductIcon product={known?.product ?? "overview"} size={32} />
                </div>
            )}
            <h3>{name}</h3>
            {unbilled ? (
                <p>
                    Google needs a billing account linked to <strong>{project}</strong> before it turns on this API. Link one, then enable it again.
                </p>
            ) : (
                <p>
                    {service === "needed" ? "An API this page needs" : "This API"} isn't enabled for <strong>{project}</strong>. Enable it to use this
                    page; it can take a minute to start working.
                </p>
            )}
            <div className="gcp-problem-actions">
                {service !== "needed" && (
                    <Button primary onClick={enable} disabled={enabling || !target}>
                        {enabling ? "Enabling…" : "Enable"}
                    </Button>
                )}
                {unbilled && <Button onClick={() => visit(consoleUrl("billing/linkedaccount", project))}>Link a billing account</Button>}
                <Button onClick={() => visit(library)}>{service === "needed" ? "Open API library" : "Open in Console"}</Button>
            </div>
        </div>
    );
}
