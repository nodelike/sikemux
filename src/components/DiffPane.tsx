import { useMemo } from "react";
import * as cmd from "../state/commands";
import { useResourceEnabled } from "../state/resources";
import { gitOverviewR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { MergeReview } from "./MergeReview";

export function DiffPane({ cwd, active }: { cwd: string; active: boolean }) {
    const overview = useResourceEnabled(active && !!cwd, gitOverviewR, cwd || "");
    const focusPath = useStore((s) => s.diffFocus[cwd] ?? null);

    const files = useMemo(() => overview.data?.status.files ?? [], [overview.data]);
    const focus = focusPath && files.some((file) => file.path === focusPath) ? focusPath : files[0]?.path;

    if (!cwd) return <div className="diff-pane-empty">open a project to review changes</div>;
    if (overview.status === "loading" && files.length === 0) return <div className="diff-pane-empty">reading the repository…</div>;
    if (overview.error) return <div className="diff-pane-empty error">{overview.error}</div>;
    if (files.length === 0) return <div className="diff-pane-empty">no changes to review</div>;

    return (
        <div className="diff-pane">
            <MergeReview
                repo={cwd}
                files={files}
                focusPath={focus}
                onOpenFile={(abs) => cmd.requestOpenFile(abs)}
                onSaved={() => void overview.refresh().catch(reportError("git refresh"))}
            />
        </div>
    );
}
