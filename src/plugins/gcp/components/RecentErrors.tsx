import { useResourceEnabled } from "../../../plugin-api/resources";
import type { Target } from "../api";
import { recentErrorsR } from "../resources";
import { ago, keyOf } from "./parts";
import { Problem } from "./Problem";

/** The newest error entries of the last hour, across the project. */
export function RecentErrors({ active, target, onOpenLogs }: { active: boolean; target: Target; onOpenLogs: () => void }) {
    const errors = useResourceEnabled(active, recentErrorsR, ...keyOf(target));
    if (errors.status === "error" && !errors.data)
        return <Problem compact error={errors.error} what="errors" onRetry={() => void errors.refresh()} />;
    if (!errors.data) return <p className="gcp-note">Loading…</p>;
    if (errors.data.length === 0) return <p className="gcp-note">No errors in the last hour.</p>;
    return (
        <div className="gcp-list">
            {errors.data.map((entry) => (
                <button key={entry.id} type="button" className="gcp-list-row" onClick={onOpenLogs}>
                    <span className="gcp-severity ERROR">!</span>
                    <span className="gcp-list-main">{entry.text}</span>
                    <span className="gcp-muted">{ago(entry.at)}</span>
                </button>
            ))}
        </div>
    );
}
