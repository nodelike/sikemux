import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Pending, Seg, StaleNotice } from "../../../plugin-api/ui";
import { kubeApi, type Revision, type RolloutHistory, type Scope } from "../api";
import { age, shortImage } from "../format";
import { kubeHistoryR, kubernetes } from "../resources";
import { kubectlScope } from "./PodView";
import { YamlView } from "./YamlView";

type WorkloadTab = "history" | "yaml";

/** An image's repository and tag; one with no tag runs latest. */
function repositoryAndTag(image: string): [string, string] {
    const colon = image.lastIndexOf(":");
    return colon > image.lastIndexOf("/") ? [image.slice(0, colon), image.slice(colon + 1)] : [image, "latest"];
}

/** What an undo changes, image by image, so the person sees what they'd be going back to. */
export function imageChange(from: Revision | undefined, to: Revision): string {
    const before = (from?.images ?? []).map(shortImage);
    const after = to.images.map(shortImage);
    if (before.join() === after.join()) return "Its images are the same as now; only the rest of its template differs.";
    if (before.length === 1 && after.length === 1) {
        const [was, wasTag] = repositoryAndTag(before[0]);
        const [will, willTag] = repositoryAndTag(after[0]);
        if (was === will) return `${was.slice(was.lastIndexOf("/") + 1)} goes from ${wasTag} to ${willTag}.`;
    }
    return `Images go from ${before.join(", ") || "none"} to ${after.join(", ") || "none"}.`;
}

export function WorkloadView({
    scope,
    kind,
    name,
    active,
    onBack,
}: {
    scope: Scope;
    kind: string;
    name: string;
    active: boolean;
    onBack: () => void;
}) {
    const [tab, setTab] = useState<WorkloadTab>("history");
    const history = useResourceEnabled(active, kubeHistoryR, scope, kind, name);
    const live = history.data;

    return (
        <section className="k8s-detail" aria-label={`${kind} ${name}`}>
            <header className="k8s-detail-head">
                <button type="button" className="k8s-back" onClick={onBack} aria-label="Back to workloads">
                    ‹ Workloads
                </button>
                <div className="k8s-detail-title">
                    <span className="k8s-chip k8s-chip-plain">{kind}</span>
                    <h2 className="k8s-name">{name}</h2>
                    {live?.paused ? <span className="k8s-chip warn">Rollout paused</span> : null}
                </div>
                {live && kind === "Deployment" ? <PauseButton scope={scope} history={live} /> : null}
            </header>
            <Seg
                label="Workload view"
                value={tab}
                onChange={setTab}
                options={[
                    { value: "history", label: "History", count: live?.revisions.length },
                    { value: "yaml", label: "YAML" },
                ]}
            />
            {tab === "yaml" ? (
                <YamlView scope={scope} kind={kind} name={name} active={active} />
            ) : !live ? (
                <Pending handle={history} loading="Reading the rollout history" rows={5} />
            ) : (
                <>
                    <StaleNotice handle={history} />
                    <Revisions scope={scope} history={live} />
                </>
            )}
        </section>
    );
}

function PauseButton({ scope, history }: { scope: Scope; history: RolloutHistory }) {
    const [busy, setBusy] = useState(false);
    const pausing = !history.paused;
    const run = async () => {
        const target = `deployment/${history.name}`;
        const yes = await confirmDialog({
            title: pausing ? `Pause ${history.name}'s rollout?` : `Resume ${history.name}'s rollout?`,
            body: pausing
                ? "Changes to it wait, unrolled, until the rollout is resumed. The pods running now keep running."
                : "Any change made while it was paused rolls out now.",
            confirmLabel: pausing ? "Pause rollout" : "Resume rollout",
            commands: [{ label: "Runs", command: ["kubectl", ...kubectlScope(scope), "rollout", pausing ? "pause" : "resume", target].join(" ") }],
        });
        if (!yes) return;
        setBusy(true);
        try {
            const done = await kubeApi.act(scope, pausing ? "pauseRollout" : "resumeRollout", history.name, "Deployment");
            notify("success", done.message);
            kubernetes.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div className="k8s-detail-actions">
            <button type="button" className="k8s-button" disabled={busy} onClick={() => void run()}>
                {pausing ? "Pause rollout" : "Resume rollout"}
            </button>
        </div>
    );
}

function Revisions({ scope, history }: { scope: Scope; history: RolloutHistory }) {
    const [busy, setBusy] = useState<number | null>(null);
    const live = history.revisions.find((revision) => revision.current);

    const undo = async (revision: Revision) => {
        const target = `${history.kind.toLowerCase()}/${history.name}`;
        const notes = [
            imageChange(live, revision),
            "Kubernetes makes it a new revision, numbered after the newest.",
            history.applied ? "It was made with kubectl apply, which this doesn't update: applying its old file again would undo this." : null,
        ].filter(Boolean);
        const yes = await confirmDialog({
            title: `Roll ${history.name} back to revision ${revision.revision}?`,
            body: notes.join("\n"),
            confirmLabel: "Roll back",
            commands: [
                {
                    label: "Runs",
                    command: ["kubectl", ...kubectlScope(scope), "rollout", "undo", target, `--to-revision=${revision.revision}`].join(" "),
                },
            ],
        });
        if (!yes) return;
        setBusy(revision.revision);
        try {
            const done = await kubeApi.act(scope, "undo", history.name, history.kind, undefined, revision.revision);
            notify("success", done.message);
            kubernetes.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };

    if (history.revisions.length === 0) return <p className="k8s-note">Kubernetes kept no revisions for it.</p>;
    return (
        <ol className="k8s-revisions" aria-label="Revisions">
            {history.revisions.map((revision) => (
                <li key={revision.revision} className={revision.current ? "current" : undefined} aria-label={`Revision ${revision.revision}`}>
                    <div className="k8s-revision-head">
                        <span className="k8s-name">Revision {revision.revision}</span>
                        {revision.current ? <span className="k8s-chip live">Live</span> : null}
                        {revision.restart ? <span className="k8s-chip">Restart</span> : null}
                        <span className="k8s-muted">{age(revision.created)} ago</span>
                        {revision.pods !== null && revision.pods > 0 ? (
                            <span className="k8s-muted">
                                {revision.pods} {revision.pods === 1 ? "pod" : "pods"}
                            </span>
                        ) : null}
                        {revision.current ? null : (
                            <button
                                type="button"
                                className="k8s-button small k8s-revision-undo"
                                disabled={busy !== null}
                                aria-label={`Roll back to revision ${revision.revision}`}
                                onClick={() => void undo(revision)}>
                                {busy === revision.revision ? "…" : "Roll back to this"}
                            </button>
                        )}
                    </div>
                    <div className="k8s-mono k8s-revision-images">{revision.images.map(shortImage).join(", ")}</div>
                    {revision.changeCause ? <div className="k8s-revision-cause">{revision.changeCause}</div> : null}
                </li>
            ))}
        </ol>
    );
}
