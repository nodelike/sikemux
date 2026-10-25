import { useEffect, useState, type ReactNode } from "react";
import { git } from "../api/git";
import { useCachedResourceEnabled } from "../state/resources";
import { gitCommitLinesR } from "../state/resources.defs";
import { ownedFile } from "./changeOwners";
import { DiffEditor } from "./DiffEditor";
import { FileReviewList } from "./FileReviewList";
import { joinPath } from "@shared/lib/paths";

const COMMIT_FILE_ESTIMATE = 240;

export function CommitReview({
    repo,
    rev,
    title,
    subtitle,
    head,
    range,
    focusPath,
    onOpenFile,
}: {
    repo: string;
    rev: string;
    title: string;
    subtitle: string;
    /** Replaces the hash-and-subject line at the top. */
    head?: ReactNode;
    /** Shows these files, open, against `base` instead of the commit's own parent. */
    range?: { base: string; files: readonly string[] };
    /** Opens this file and scrolls to it. */
    focusPath?: string | null;
    onOpenFile: (abs: string) => void;
}) {
    const [files, setFiles] = useState<string[]>([]);
    const rangeFiles = range?.files;
    const lines = useCachedResourceEnabled(!range, gitCommitLinesR, repo, rev).data;

    useEffect(() => {
        if (rangeFiles) {
            setFiles([...rangeFiles]);
            return;
        }
        let cancelled = false;
        setFiles([]);
        git.commitFiles(repo, rev)
            .then((found) => {
                if (!cancelled) setFiles(found);
            })
            .catch(() => {
                if (!cancelled) setFiles([]);
            });
        return () => {
            cancelled = true;
        };
    }, [repo, rev, rangeFiles]);

    return (
        <div className="commit-review">
            {head ?? (
                <div className="commit-head">
                    <span className="commit-hash">{title}</span>
                    <span className="commit-subject">{subtitle}</span>
                </div>
            )}
            {files.length === 0 ? (
                <div className="commit-empty">no files</div>
            ) : (
                <FileReviewList
                    paths={files}
                    focusPath={focusPath ?? undefined}
                    startCollapsed={!range}
                    estimate={() => COMMIT_FILE_ESTIMATE}
                    onOpenFile={(path) => onOpenFile(joinPath(repo, path))}
                    status={() => null}
                    body={(path) => (
                        <DiffEditor
                            repo={repo}
                            path={path}
                            baseRev={range?.base ?? `${rev}~1`}
                            headRev={rev}
                            editable={false}
                            autoHeight
                            owned={range ? undefined : ownedFile(lines, path)}
                        />
                    )}
                />
            )}
        </div>
    );
}
