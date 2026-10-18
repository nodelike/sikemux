import { useState } from "react";
import { useResourceEnabled } from "../../plugin-api/resources";
import { IconAlertCircle, IconInfo, IconWarning } from "../../plugin-api/ui";
import type { RepoRef } from "../api";
import { useOpenLocalFile } from "../localRepo";
import { annotationsR } from "../resources";

/** How many of a job's annotations show before the rest fold behind a count. */
const SHOWN = 3;

const LEVEL_LABEL: Record<string, string> = { failure: "Error", warning: "Warning", notice: "Notice" };

function LevelIcon({ level }: { level: string }) {
    if (level === "failure") return <IconAlertCircle size={14} />;
    if (level === "warning") return <IconWarning size={14} />;
    return <IconInfo size={14} />;
}

export function annotationPlace(path: string | null, startLine: number | null): string | null {
    if (!path) return null;
    return startLine ? `${path}:${startLine}` : path;
}

interface Props {
    repo: RepoRef;
    checkRunId: string;
    active: boolean;
}

/** What the run flagged in a job, a row each, under the job's band. */
export function Annotations({ repo, checkRunId, active }: Props) {
    const found = useResourceEnabled(active, annotationsR, repo, checkRunId);
    const openFile = useOpenLocalFile();
    const [everything, setEverything] = useState(false);
    const annotations = found.data ?? [];
    if (annotations.length === 0) return null;
    const shown = everything ? annotations : annotations.slice(0, SHOWN);
    const rest = annotations.length - shown.length;

    return (
        <div className="gha-annotations">
            {shown.map((annotation, index) => {
                const place = annotationPlace(annotation.path, annotation.startLine);
                return (
                    <div className="gha-annotation" key={`${annotation.path}-${annotation.startLine}-${index}`} data-level={annotation.level}>
                        <span className="gha-annotation-icon" role="img" aria-label={LEVEL_LABEL[annotation.level] ?? annotation.level}>
                            <LevelIcon level={annotation.level} />
                        </span>
                        <span className="gha-annotation-text">
                            {annotation.title && <span className="gha-annotation-title">{annotation.title} </span>}
                            {annotation.message}
                        </span>
                        {place && annotation.path && openFile ? (
                            <button
                                type="button"
                                className="gha-annotation-place gha-mono"
                                title="Open it in the editor"
                                onClick={() => openFile(annotation.path ?? "", annotation.startLine)}>
                                {place}
                            </button>
                        ) : (
                            place && <span className="gha-annotation-place gha-mono">{place}</span>
                        )}
                    </div>
                );
            })}
            {(rest > 0 || everything) && annotations.length > SHOWN && (
                <button type="button" className="gha-annotation gha-annotation-more" onClick={() => setEverything((was) => !was)}>
                    {everything ? "Show fewer" : `${rest} more`}
                </button>
            )}
        </div>
    );
}
