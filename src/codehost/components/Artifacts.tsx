import { useState } from "react";
import { notify, reportError } from "../../plugin-api/host";
import { useResourceEnabled } from "../../plugin-api/resources";
import { hostApi, type RepoRef } from "../api";
import { artifactsR } from "../resources";

export function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    const units = ["KB", "MB", "GB"];
    let value = bytes / 1024;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024;
        unit += 1;
    }
    return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

interface Props {
    repo: RepoRef;
    runId: string;
    active: boolean;
}

export function Artifacts({ repo, runId, active }: Props) {
    const found = useResourceEnabled(active, artifactsR, repo, runId);
    const [saving, setSaving] = useState<string | null>(null);
    const artifacts = found.data ?? [];
    if (artifacts.length === 0) return null;

    const save = (id: string, name: string) => {
        setSaving(id);
        void hostApi(repo.provider)
            .downloadArtifact(repo, id, name)
            .then((saved) => notify("success", `Saved ${name} to ${saved.path}`))
            .catch(reportError(`Could not download ${name}`))
            .finally(() => setSaving(null));
    };

    return (
        <div className="gha-artifacts">
            {artifacts.map((artifact) => (
                <div className="gha-artifact" key={artifact.id}>
                    <span className="gha-artifact-name">{artifact.name}</span>
                    <span className="gha-dim">{formatBytes(artifact.sizeBytes)}</span>
                    {artifact.expired ? (
                        <span className="gha-dim">expired</span>
                    ) : (
                        <button type="button" className="gha-link" disabled={saving === artifact.id} onClick={() => save(artifact.id, artifact.name)}>
                            {saving === artifact.id ? "Saving…" : "Download"}
                        </button>
                    )}
                </div>
            ))}
        </div>
    );
}
