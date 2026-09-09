import { useMemo } from "react";
import * as cmd from "../state/commands";
import { useResourceEnabled } from "../state/resources";
import { gitOverviewR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { CommitReview } from "./CommitReview";
import { MergeReview } from "./MergeReview";
import { invalidateDiffContentCache } from "./DiffEditor";
import { DiffWorkerProvider } from "./DiffWorkerProvider";

export function DiffPane({ cwd, active }: { cwd: string; active: boolean }) {
    return (
        <DiffWorkerProvider>
            <DiffPaneContent cwd={cwd} active={active} />
        </DiffWorkerProvider>
    );
}

function DiffPaneContent({ cwd, active }: { cwd: string; active: boolean }) {
    const overview = useResourceEnabled(active && !!cwd, gitOverviewR, cwd || "");
    const target = useStore((s) => s.diffTarget[cwd] ?? null);
    const files = useMemo(() => overview.data?.status.files ?? [], [overview.data]);

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
                onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                onSaved={() => {
                    invalidateDiffContentCache(cwd);
                    void overview.refresh().catch(reportError("git refresh"));
                }}
            />
        </div>
    );
}
