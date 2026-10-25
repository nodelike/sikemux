import { useMemo, useRef, type ReactNode } from "react";
import { DiffEditor } from "./DiffEditor";
import { FileReviewList } from "./FileReviewList";
import { hasUnstaged, isStaged, type ChangeOwners, type GitFile } from "../api/git";
import { joinPath } from "@shared/lib/paths";
import { gitFileBadges, gitStatusBadge, type GitStatusBadge } from "./gitFileStatus";
import { ownedFile, type OwnedFile } from "./changeOwners";

const REVIEW_ROW_ESTIMATE = 250;
const REVIEW_DOUBLE_ROW_ESTIMATE = 470;

function sameFileList(a: readonly GitFile[], b: readonly GitFile[]): boolean {
    if (a === b) return true;
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
        if (a[i].path !== b[i].path || a[i].index !== b[i].index || a[i].worktree !== b[i].worktree) return false;
    }
    return true;
}

/** A git refresh hands us a brand new array even when nothing changed; reuse
 *  the previous one so the diffs below don't remount. */
function useStableFileList(next: GitFile[]): GitFile[] {
    const held = useRef(next);
    if (!sameFileList(held.current, next)) held.current = next;
    return held.current;
}

export function MergeReview({
    repo,
    files: incomingFiles,
    focusPath,
    onOpenFile,
    onSaved,
    fileActions,
    owners,
    stagedOwners,
    foldAll,
}: {
    repo: string;
    files: GitFile[];
    focusPath?: string;
    onOpenFile: (abs: string) => void;
    onSaved: () => void;
    /** Buttons shown in a file's header while the pointer is on it. */
    fileActions?: (file: GitFile) => ReactNode;
    /** Who wrote each changed line, against the index and against HEAD. */
    owners?: ChangeOwners;
    stagedOwners?: ChangeOwners;
    foldAll?: { folded: boolean; seq: number };
}) {
    const files = useStableFileList(incomingFiles);
    const byPath = useMemo(() => new Map(files.map((file) => [file.path, file])), [files]);
    const paths = useMemo(() => files.map((file) => file.path), [files]);
    return (
        <FileReviewList
            paths={paths}
            focusPath={focusPath}
            foldAll={foldAll}
            estimate={(path) => {
                const file = byPath.get(path);
                return file && isStaged(file) && hasUnstaged(file) ? REVIEW_DOUBLE_ROW_ESTIMATE : REVIEW_ROW_ESTIMATE;
            }}
            onOpenFile={(path) => onOpenFile(joinPath(repo, path))}
            status={(path) => {
                const file = byPath.get(path);
                return file ? gitFileBadges(file).map((badge) => <GitStatusSymbol key={badge.source} badge={badge} />) : null;
            }}
            actions={
                fileActions
                    ? (path) => {
                          const file = byPath.get(path);
                          return file ? fileActions(file) : null;
                      }
                    : undefined
            }
            body={(path, focused) => {
                const file = byPath.get(path);
                return file ? (
                    <MergeFileDiff
                        repo={repo}
                        file={file}
                        stagedOwned={ownedFile(stagedOwners, path)}
                        owned={ownedFile(owners, path)}
                        editable={focused && hasUnstaged(file)}
                        onSaved={onSaved}
                    />
                ) : null;
            }}
        />
    );
}

function GitStatusSymbol({ badge }: { badge: GitStatusBadge | null }) {
    if (!badge) return null;
    const description = badge.source === badge.label ? badge.label : `${badge.source}: ${badge.label}`;
    return (
        <span className={`git-status-symbol git-${badge.cls}`} title={description} aria-label={description}>
            {badge.letter}
        </span>
    );
}

function MergeFileDiff({
    repo,
    file,
    stagedOwned,
    owned,
    editable,
    onSaved,
}: {
    repo: string;
    file: GitFile;
    stagedOwned: OwnedFile | undefined;
    owned: OwnedFile | undefined;
    editable: boolean;
    onSaved: () => void;
}) {
    const path = file.path;
    const staged = isStaged(file);
    const unstaged = hasUnstaged(file);
    const indexBadge = gitStatusBadge(file.index, "staged");
    const worktreeBadge = gitStatusBadge(file.worktree, "unstaged");

    return (
        <div className="merge-review-content">
            {staged && unstaged ? (
                <div className="merge-sections">
                    <div className="merge-section">
                        <div className="merge-section-title">
                            <GitStatusSymbol badge={indexBadge} />
                            <span>staged</span>
                        </div>
                        <DiffEditor repo={repo} path={path} baseRev="HEAD" headRev=":index" editable={false} autoHeight owned={stagedOwned} />
                    </div>
                    <div className="merge-section">
                        <div className="merge-section-title">
                            <GitStatusSymbol badge={worktreeBadge} />
                            <span>unstaged</span>
                        </div>
                        <DiffEditor repo={repo} path={path} baseRev=":index" editable={editable} onSaved={onSaved} autoHeight owned={owned} />
                    </div>
                </div>
            ) : staged ? (
                <DiffEditor repo={repo} path={path} baseRev="HEAD" headRev=":index" editable={false} autoHeight owned={owned} />
            ) : (
                <DiffEditor repo={repo} path={path} baseRev="HEAD" editable={editable} onSaved={onSaved} autoHeight owned={owned} />
            )}
        </div>
    );
}
