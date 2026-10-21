import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Pending, StaleNotice } from "../../../plugin-api/ui";
import { kubeApi, type HelmDetail, type HelmRelease, type HelmStep, type Scope } from "../api";
import { age, type Tone } from "../format";
import { kubeHelmReleaseR, kubernetes } from "../resources";
import { Dot } from "./parts";

/** The colour a release's status reads in: failed red, mid-change amber, replaced grey. */
export function helmTone(status: string): Tone {
    if (status === "deployed") return "live";
    if (status === "failed") return "danger";
    if (status.startsWith("pending") || status === "uninstalling") return "warn";
    return "idle";
}

function chartLine(release: Pick<HelmRelease, "chart" | "chartVersion">): string {
    return release.chartVersion ? `${release.chart} ${release.chartVersion}` : release.chart;
}

export function HelmReleases({
    releases,
    find,
    showNamespace,
    onOpen,
}: {
    releases: HelmRelease[];
    find: string;
    showNamespace: boolean;
    onOpen: (release: HelmRelease) => void;
}) {
    const words = find.trim().toLowerCase();
    const shown = releases.filter((release) => !words || release.name.includes(words) || release.chart.toLowerCase().includes(words));
    if (releases.length === 0) return <p className="k8s-note">No Helm releases here.</p>;
    if (shown.length === 0) return <p className="k8s-note">No releases match.</p>;
    return (
        <section className="k8s-section" aria-label="Helm releases">
            <table className="k8s-table" aria-label="Helm releases">
                <thead>
                    <tr>
                        <th aria-label="Status" />
                        <th>Release</th>
                        <th>Status</th>
                        <th>Chart</th>
                        <th>App version</th>
                        <th className="k8s-num">Revision</th>
                        <th>Updated</th>
                    </tr>
                </thead>
                <tbody>
                    {shown.map((release) => (
                        <tr
                            key={`${release.namespace}/${release.name}`}
                            className="k8s-row"
                            tabIndex={0}
                            aria-label={release.name}
                            onClick={() => onOpen(release)}
                            onKeyDown={(event) => {
                                if (event.key === "Enter") onOpen(release);
                            }}>
                            <td className="k8s-cell-dot">
                                <Dot tone={helmTone(release.status)} label={release.status} />
                            </td>
                            <td>
                                <span className="k8s-name">{release.name}</span>
                                {showNamespace ? <span className="k8s-chip">{release.namespace}</span> : null}
                            </td>
                            <td className={`k8s-status ${helmTone(release.status)}`}>{release.status}</td>
                            <td className="k8s-mono">{chartLine(release)}</td>
                            <td className="k8s-muted k8s-mono">{release.appVersion ?? "—"}</td>
                            <td className="k8s-num">{release.revision}</td>
                            <td className="k8s-muted">{age(release.updated)} ago</td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </section>
    );
}

/** Whether a step can be rolled back to: an earlier revision that once ran, not the one in place now. */
export function canRollBackTo(step: HelmStep, newest: number): boolean {
    return step.revision !== newest && (step.status === "superseded" || step.status === "deployed");
}

export function HelmReleaseView({ scope, name, active, onBack }: { scope: Scope; name: string; active: boolean; onBack: () => void }) {
    const release = useResourceEnabled(active, kubeHelmReleaseR, scope, name);
    const detail = release.data;
    const newest = detail?.history[0];
    return (
        <section className="k8s-detail" aria-label={`Helm release ${name}`}>
            <header className="k8s-detail-head">
                <button type="button" className="k8s-back" onClick={onBack} aria-label="Back to Helm releases">
                    ‹ Helm
                </button>
                <div className="k8s-detail-title">
                    {newest ? <Dot tone={helmTone(newest.status)} label={newest.status} /> : null}
                    <h2 className="k8s-name">{name}</h2>
                    {newest ? <span className={`k8s-status ${helmTone(newest.status)}`}>{newest.status}</span> : null}
                    {newest ? <span className="k8s-muted k8s-mono">{chartLine(newest)}</span> : null}
                </div>
            </header>
            {!detail ? (
                <Pending handle={release} loading="Reading the release" rows={5} />
            ) : (
                <>
                    <StaleNotice handle={release} />
                    <Steps scope={scope} name={name} detail={detail} />
                    <h3 className="k8s-heading">What it made</h3>
                    {detail.made.length === 0 ? (
                        <p className="k8s-note">Its manifest names no objects.</p>
                    ) : (
                        <ul className="k8s-pairs" aria-label="What it made">
                            {detail.made.map((made) => (
                                <li key={`${made.kind}/${made.namespace ?? ""}/${made.name}`}>
                                    <span className="k8s-muted">{made.kind}</span>
                                    <span className="k8s-mono">{made.name}</span>
                                    {made.namespace ? <span className="k8s-chip">{made.namespace}</span> : null}
                                </li>
                            ))}
                        </ul>
                    )}
                    <p className="k8s-muted">Values and chart notes aren't shown: charts often put passwords in them.</p>
                </>
            )}
        </section>
    );
}

function Steps({ scope, name, detail }: { scope: Scope; name: string; detail: HelmDetail }) {
    const [busy, setBusy] = useState<number | null>(null);
    const newest = detail.history[0]?.revision ?? 0;

    const rollBack = async (step: HelmStep) => {
        const flags = [scope.context ? `--kube-context=${scope.context}` : null, scope.namespace ? `--namespace=${scope.namespace}` : null].filter(
            Boolean,
        );
        const yes = await confirmDialog({
            title: `Roll ${name} back to revision ${step.revision}?`,
            body: `Helm makes it revision ${newest + 1}, with revision ${step.revision}'s chart and values.`,
            confirmLabel: "Roll back",
            commands: [{ label: "Runs", command: ["helm", "rollback", name, String(step.revision), ...flags].join(" ") }],
        });
        if (!yes) return;
        setBusy(step.revision);
        try {
            const done = await kubeApi.helmRollback(scope, name, step.revision);
            notify("success", done.message);
            kubernetes.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };

    return (
        <ol className="k8s-revisions" aria-label="Release history">
            {detail.history.map((step) => (
                <li key={step.revision} className={step.revision === newest ? "current" : undefined} aria-label={`Revision ${step.revision}`}>
                    <div className="k8s-revision-head">
                        <span className="k8s-name">Revision {step.revision}</span>
                        <span className={`k8s-status ${helmTone(step.status)}`}>{step.status}</span>
                        <span className="k8s-muted k8s-mono">{chartLine(step)}</span>
                        {step.appVersion ? <span className="k8s-muted">app {step.appVersion}</span> : null}
                        <span className="k8s-muted">{age(step.updated)} ago</span>
                        {canRollBackTo(step, newest) ? (
                            <button
                                type="button"
                                className="k8s-button small k8s-revision-undo"
                                disabled={busy !== null}
                                aria-label={`Roll back to revision ${step.revision}`}
                                onClick={() => void rollBack(step)}>
                                {busy === step.revision ? "…" : "Roll back to this"}
                            </button>
                        ) : null}
                    </div>
                    {step.description ? (
                        <div className={step.status === "failed" ? "k8s-revision-cause failed" : "k8s-revision-cause"}>{step.description}</div>
                    ) : null}
                </li>
            ))}
        </ol>
    );
}
