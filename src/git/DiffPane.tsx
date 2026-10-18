import { useMemo } from "react";
import { hasUnstaged, isStaged } from "../api/git";
import * as cmd from "../state/commands";
import { useCachedResourceEnabled } from "../state/resources";
import { gitOverviewR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { CommitReview } from "./CommitReview";
import { MergeReview } from "./MergeReview";
import { useChangeOwners } from "./changeOwners";
import { useProjectRepo } from "./projectRepo";
import { invalidateDiffContentCache } from "./DiffEditor";

export function DiffPane({ cwd: project, visible }: { cwd: string; visible: boolean }) {
    const target = useStore((s) => s.diffTarget[project] ?? null);
    const cwd = useProjectRepo(project, visible).repo ?? project;
    const overview = useCachedResourceEnabled(visible && !!cwd, gitOverviewR, cwd || "");
    const files = useMemo(() => overview.data?.status.files ?? [], [overview.data]);
    const owners = useChangeOwners(cwd, false, visible && !!cwd);
    const stagedOwners = useChangeOwners(cwd, true, visible && files.some((file) => isStaged(file) && hasUnstaged(file)));

    if (!cwd) return <div className="diff-pane-empty">open a project to review changes</div>;
    if (overview.error) return <div className="diff-pane-empty error">{overview.error}</div>;

    if (target?.kind === "commit") {
        return (
            <div className="diff-pane">
                <CommitReview
                    key={target.rev}
                    repo={cwd}
                    rev={target.rev}
                    title={target.rev}
                    subtitle={target.subject}
                    onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                />
            </div>
        );
    }

    if (overview.status === "loading" && files.length === 0) return <div className="diff-pane-empty">reading the repository…</div>;
    if (files.length === 0) return <div className="diff-pane-empty">no changes to review</div>;

    const focus = target && files.some((file) => file.path === target.path) ? target.path : files[0]?.path;
    return (
        <div className="diff-pane">
            <MergeReview
                repo={cwd}
                files={files}
                focusPath={focus}
                owners={owners}
                stagedOwners={stagedOwners}
                onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                onSaved={() => {
                    invalidateDiffContentCache(cwd);
                    void overview.refresh().catch(reportError("git refresh"));
                }}
            />
        </div>
    );
}
