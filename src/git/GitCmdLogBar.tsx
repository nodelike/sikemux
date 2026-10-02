import { clearGitCmdLog, toggleGitCmdLog } from "../state/git";
import { useStore } from "../state/store";
import { Tooltip } from "../ui/Tooltip";

export function GitCmdLogBar() {
    const open = useStore((s) => s.gitCmdLogOpen);
    const log = useStore((s) => s.gitCmdLog);

    if (!open) {
        return (
            <Tooltip label="Show command log (@)">
                <button type="button" className="git-log-bar collapsed" onClick={toggleGitCmdLog}>
                    <span className="kbd">@</span>
                    <span className="git-log-bar-summary">{log.length === 0 ? "no commands yet" : `${log.length} cmds`}</span>
                </button>
            </Tooltip>
        );
    }

    return (
        <div className="git-log-bar open">
            <div className="git-log-bar-head">
                <span className="kbd">@</span>
                <span className="git-log-bar-title">command log</span>
                <span className="git-log-bar-count">{log.length}</span>
                <Tooltip label="Clear log">
                    <button type="button" className="git-log-bar-btn" onClick={clearGitCmdLog} disabled={log.length === 0}>
                        clear
                    </button>
                </Tooltip>
                <Tooltip label="Hide log (@)">
                    <button type="button" className="git-log-bar-btn" onClick={toggleGitCmdLog}>
                        close
                    </button>
                </Tooltip>
            </div>
            <div className="git-log-bar-rows">
                {log.length === 0 && <div className="git-log-bar-empty">log empty — every git command will appear here</div>}
                {log
                    .slice()
                    .reverse()
                    .map((e) => (
                        <div key={e.id} className={`git-log-row status-${e.status}`}>
                            <span className="git-log-row-time">{formatTime(e.ts)}</span>
                            <span className="git-log-row-glyph">{e.status === "running" ? "…" : e.status === "ok" ? "✓" : "✗"}</span>
                            <span className="git-log-row-label">{e.label}</span>
                            {e.detail && <span className="git-log-row-detail">{firstLine(e.detail)}</span>}
                        </div>
                    ))}
            </div>
        </div>
    );
}

function formatTime(ts: number): string {
    const d = new Date(ts);
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
const pad = (n: number) => String(n).padStart(2, "0");

function firstLine(s: string): string {
    const nl = s.indexOf("\n");
    return nl === -1 ? s : s.slice(0, nl);
}
