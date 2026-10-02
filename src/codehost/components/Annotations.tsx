import { useResourceEnabled } from "../../plugin-api/resources";
import type { RepoRef } from "../api";
import { useOpenLocalFile } from "../localRepo";
import { annotationsR } from "../resources";

const TONE: Record<string, string | undefined> = { failure: "danger", warning: "warn" };

export function annotationPlace(path: string | null, startLine: number | null): string | null {
    if (!path) return null;
    return startLine ? `${path}:${startLine}` : path;
}

interface Props {
    repo: RepoRef;
    checkRunId: string;
    active: boolean;
}

export function Annotations({ repo, checkRunId, active }: Props) {
    const found = useResourceEnabled(active, annotationsR, repo, checkRunId);
    const openFile = useOpenLocalFile();
    const annotations = found.data ?? [];
    if (annotations.length === 0) return null;

    return (
        <div className="gha-annotations">
            {annotations.map((annotation, index) => {
                const place = annotationPlace(annotation.path, annotation.startLine);
                return (
                    <div className="gha-callout" key={`${annotation.path}-${annotation.startLine}-${index}`} data-tone={TONE[annotation.level]}>
                        <span className="gha-annotation-level">{annotation.level}</span>
                        <div className="gha-annotation-body">
                            {annotation.title && <span className="gha-annotation-title">{annotation.title}</span>}
                            <span className="gha-annotation-message">{annotation.message}</span>
                            {place && annotation.path && openFile ? (
                                <button
                                    type="button"
                                    className="gha-link gha-annotation-place gha-mono"
                                    title="Open it in the editor"
                                    onClick={() => openFile(annotation.path ?? "", annotation.startLine)}>
                                    {place}
                                </button>
                            ) : (
                                place && <span className="gha-annotation-place gha-mono">{place}</span>
                            )}
                        </div>
                    </div>
                );
            })}
        </div>
    );
}
