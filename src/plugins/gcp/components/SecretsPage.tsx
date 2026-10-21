import { useResourceEnabled } from "../../../plugin-api/resources";
import { SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type Secret, type Target } from "../api";
import { secretR, secretsR } from "../resources";
import { updateGcpView, useGcpView } from "../state";
import type { ProductProps } from "./GcpPane";
import { Filter } from "./RunPage";
import { ago, Button, consoleUrl, Facts, keyOf, Loaded, PageHeader, Status, Table, useAction, visit } from "./parts";
import { Problem } from "./Problem";

const replication = (secret: Secret) => (secret.replication === "automatic" ? "Automatic" : secret.replication.join(", "));

/** A rotation period such as `2592000s` in days. */
export function period(seconds: string | null): string | null {
    if (!seconds) return null;
    const value = Number(seconds.replace(/s$/, ""));
    if (!Number.isFinite(value)) return seconds;
    const days = value / 86_400;
    return Number.isInteger(days) ? `Every ${days} day${days === 1 ? "" : "s"}` : `Every ${Math.round(value / 3600)} hours`;
}

export function SecretsPage({ paneId, active, target }: ProductProps) {
    const view = useGcpView(paneId);
    const secrets = useResourceEnabled(active, secretsR, ...keyOf(target));
    const words = view.filter.trim().toLowerCase();
    const open = (name: string) => updateGcpView(paneId, { open: view.open === name ? null : name });
    return (
        <div className="gcp-page">
            <PageHeader title="Secret Manager">
                <Button onClick={() => visit(consoleUrl("security/secret-manager", target.project))}>Create secret in Cloud console</Button>
            </PageHeader>
            <p className="gcp-note">Names and versions only. Secret values are never read here, nor given to agents.</p>
            <Filter paneId={paneId} placeholder="Filter secrets" />
            <Loaded handle={secrets} label="secrets" empty={(rows) => (rows.length === 0 ? "No secrets in this project." : null)}>
                {(rows) => (
                    <div className={view.open ? "gcp-split" : undefined}>
                        <Table
                            label="Secrets"
                            rows={rows.filter((secret) => !words || secret.name.toLowerCase().includes(words))}
                            rowKey={(secret) => secret.name}
                            selected={view.open}
                            onOpen={(secret: Secret) => open(secret.name)}
                            columns={[
                                { label: "Name", width: "minmax(160px, 1.4fr)", cell: (secret) => <span className="gcp-name">{secret.name}</span> },
                                { label: "Replication", cell: replication },
                                { label: "Rotation", cell: (secret) => period(secret.rotationPeriod) ?? "—" },
                                { label: "Created", cell: (secret) => ago(secret.createdAt) },
                            ]}
                        />
                        {view.open && <SecretPanel active={active} target={target} name={view.open} onClose={() => open(view.open!)} />}
                    </div>
                )}
            </Loaded>
        </div>
    );
}

function SecretPanel({ active, target, name, onClose }: { active: boolean; target: Target; name: string; onClose: () => void }) {
    const secret = useResourceEnabled(active, secretR, ...keyOf(target), name);
    const action = useAction();
    const set = (version: string, enabled: boolean) =>
        void action.run(
            {
                title: `${enabled ? "Enable" : "Disable"} version ${version} of ${name}?`,
                body: enabled
                    ? `Version ${version} can be read again.`
                    : `Anything that reads version ${version}, or "latest" while it is the newest, fails until it is enabled again.`,
                confirmLabel: enabled ? "Enable" : "Disable",
                destructive: !enabled,
            },
            `Version ${version} is ${enabled ? "enabled" : "disabled"}`,
            () => gcpApi.secretVersion(target, name, version, enabled),
        );
    return (
        <aside className="gcp-panel" aria-label={`Secret ${name}`}>
            <header>
                <h2>{name}</h2>
                <button type="button" className="gcp-icon-button" aria-label="Close" onClick={onClose}>
                    ✕
                </button>
            </header>
            {secret.status === "error" && !secret.data ? (
                <Problem error={secret.error} what="the secret" onRetry={() => void secret.refresh()} />
            ) : !secret.data ? (
                <SkeletonRows rows={4} label="Loading the secret" />
            ) : (
                <>
                    <Facts
                        items={[
                            ["Replication", replication(secret.data)],
                            ["Rotation", period(secret.data.rotationPeriod)],
                            ["Next rotation", secret.data.nextRotation && ago(secret.data.nextRotation)],
                            ["Expires", secret.data.expiresAt && ago(secret.data.expiresAt)],
                        ]}
                    />
                    <Table
                        label="Versions"
                        rows={secret.data.versions}
                        rowKey={(version) => version.version}
                        columns={[
                            { label: "", width: "24px", cell: (version) => <Status state={version.state} /> },
                            { label: "Version", cell: (version) => version.version },
                            { label: "Created", cell: (version) => ago(version.createdAt) },
                            {
                                label: "",
                                width: "90px",
                                align: "end",
                                cell: (version) =>
                                    version.state === "ENABLED" ? (
                                        <Button danger disabled={action.busy} onClick={() => set(version.version, false)}>
                                            Disable
                                        </Button>
                                    ) : version.state === "DISABLED" ? (
                                        <Button disabled={action.busy} onClick={() => set(version.version, true)}>
                                            Enable
                                        </Button>
                                    ) : null,
                            },
                        ]}
                    />
                </>
            )}
        </aside>
    );
}
