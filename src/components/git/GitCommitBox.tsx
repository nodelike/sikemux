import type { RefObject } from "react";
import { PRIMARY_SHORTCUT } from "../../lib/platform";
import { setGitDraft, useGitWorkbench } from "../../state/gitWorkbench";
import { IconCommit } from "../Icons";

export function GitCommitBox({
    repo,
    busy,
    inputRef,
    onCommit,
}: {
    repo: string;
    busy: boolean;
    inputRef: RefObject<HTMLTextAreaElement | null>;
    onCommit: () => void;
}) {
    const text = useGitWorkbench((state) => state.drafts[repo] ?? "");
    return (
        <div className="git-cp-body">
            <textarea
                ref={inputRef}
                className="git-cp-input"
                placeholder="commit message…"
                value={text}
                spellCheck={false}
                readOnly={busy}
                rows={3}
                onChange={(e) => setGitDraft(repo, e.target.value)}
                onKeyDown={(e) => {
                    if (busy) e.preventDefault();
                    else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) onCommit();
                    else if (e.key === "Escape") {
                        setGitDraft(repo, "");
                        inputRef.current?.blur();
                    }
                    e.stopPropagation();
                }}
            />
            <div className="git-cp-actions">
                <button
                    className="git-cp-commit"
                    type="button"
                    disabled={busy || !text.trim()}
                    onClick={onCommit}
                    title={`Commit staged (${PRIMARY_SHORTCUT}⏎)`}>
                    <IconCommit size={13} />
                    commit
                </button>
            </div>
        </div>
    );
}
