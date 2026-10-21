import { useEffect, useRef, useState } from "react";
import type { PluginStream } from "../../../plugin-api/backend";
import { basename, confirmDialog, pickFolder, swallow, useNearestProject } from "../../../plugin-api/host";
import { Seg } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type DeployProgress, type VercelProject } from "../api";
import { vercel, vercelDeploymentsR, vercelProjectsR } from "../resources";

const TARGETS = [
    { value: "preview" as const, label: "Preview" },
    { value: "production" as const, label: "Production" },
];

export function progressLabel(progress: DeployProgress | null): string {
    if (!progress) return "";
    if (progress.step === "Uploading") return progress.total === 0 ? "Uploading" : `Uploading ${progress.done} of ${progress.total} files`;
    return progress.step;
}

/** Deploys a folder on this Mac to the project, as `vercel deploy` does, then opens the deployment. */
export function DeployFolder({
    team,
    project,
    onDeployed,
    onClose,
}: {
    team: string;
    project: VercelProject;
    onDeployed: (id: string) => void;
    onClose: () => void;
}) {
    const nearest = useNearestProject();
    const [folder, setFolder] = useState<string | null>(nearest?.cwd ?? null);
    const [target, setTarget] = useState<"preview" | "production">("preview");
    const [progress, setProgress] = useState<DeployProgress | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [running, setRunning] = useState(false);
    const stream = useRef<PluginStream | null>(null);
    useEffect(() => () => stream.current?.stop(), []);

    const deploy = async () => {
        if (!folder) return;
        if (target === "production") {
            const sure = await confirmDialog({
                title: `Deploy ${basename(folder)} to production?`,
                body: `Once it builds, ${project.name}'s production domains serve it, uncommitted changes included.`,
                confirmLabel: "Deploy to production",
                destructive: true,
            });
            if (!sure) return;
        }
        setError(null);
        setRunning(true);
        setProgress({ step: "Reading files", done: 0, total: 0, deployment: null });
        stream.current = vercelApi.deployFolder(team, project.id, folder, target === "production", {
            onItem: (next) => {
                setProgress(next);
                if (next.deployment) {
                    setRunning(false);
                    vercel.refresh(vercelDeploymentsR, vercelProjectsR);
                    onDeployed(next.deployment.id);
                }
            },
            onError: (failure) => {
                setError(errorText(failure));
                setRunning(false);
            },
            onEnd: () => setRunning(false),
        });
    };

    return (
        <section className="vercel-box vercel-form-box vercel-deploy" aria-label="Deploy a folder">
            <div className="vercel-domain-head">
                <span className="vercel-box-title">Deploy from this Mac</span>
                <button type="button" className="vercel-link" onClick={onClose}>
                    Close
                </button>
            </div>
            <div className="vercel-form-row">
                <span>Folder</span>
                <span className="vercel-deploy-folder">
                    <span className="vercel-mono" title={folder ?? undefined}>
                        {folder ?? "None chosen"}
                    </span>
                    <button
                        type="button"
                        className="vercel-button"
                        disabled={running}
                        onClick={() =>
                            void pickFolder(`Deploy to ${project.name}`)
                                .then((picked) => {
                                    if (picked) setFolder(picked);
                                })
                                .catch(swallow("choose a folder"))
                        }>
                        Choose…
                    </button>
                </span>
            </div>
            <div className="vercel-form-row">
                <span>Environment</span>
                <Seg label="Environment" options={TARGETS} value={target} onChange={setTarget} />
            </div>
            <div className="vercel-form-foot">
                <span className="vercel-muted" role="status">
                    {error ?? (progress ? progressLabel(progress) : "Files git ignores, and every .env file, stay on this Mac.")}
                </span>
                <button type="button" className="vercel-button primary" disabled={!folder || running} onClick={() => void deploy()}>
                    {running ? "Deploying…" : "Deploy"}
                </button>
            </div>
        </section>
    );
}
