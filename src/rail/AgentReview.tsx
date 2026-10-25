import { useEffect, useState } from "react";
import { coreCall } from "../api/core";
import { sendToAgent } from "../agents/sendToAgent";
import { joinPath } from "@shared/lib/paths";
import * as cmd from "../state/commands";
import { requestOpenFile } from "../state/commands/editor";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import type { AgentWorktree } from "../state/types";
import { IconCheck, IconClose, IconExternal } from "@shared/ui/Icons";

const SHOWN_FILES = 6;

/** What an agent's worktree shows of its work, as the core works it out. */
interface Review {
    readonly changed: readonly string[];
    readonly uncommitted: number;
    readonly summary: string | null;
    readonly tested: boolean;
    readonly passed: number;
    readonly failed: number;
    readonly url: string | null;
    /** What to tell the agent about the tests that fail, while some do. */
    readonly failures: string | null;
}

export function AgentReview({
    agentId,
    worktree,
    anchor,
    onClose,
}: {
    agentId: string;
    worktree: AgentWorktree;
    anchor: DOMRect;
    onClose: () => void;
}) {
    const title = useStore((state) => state.agents[agentId]?.title || "Agent");
    const display = useStore((state) => state.agentStatus[agentId]);
    const [evidence, setEvidence] = useState<Review | null>(null);

    useEffect(() => {
        let cancelled = false;
        void coreCall<Review>("agentReview", { agentId })
            .then((found) => {
                if (!cancelled) setEvidence(found);
            })
            .catch(swallow("agent review"));
        return () => {
            cancelled = true;
        };
    }, [agentId, worktree]);

    const { passed = 0, failed = 0, summary = null, failures = null } = evidence ?? {};
    const left = Math.min(anchor.left, window.innerWidth - 372);

    return (
        <>
            <div className="env-dd-scrim" onClick={onClose} />
            <div
                className="agent-review"
                role="dialog"
                aria-label={`${title}'s work`}
                data-overlay
                style={{ top: anchor.bottom + 6, left: Math.max(8, left) }}
                onKeyDown={(event) => {
                    if (event.key === "Escape") onClose();
                }}>
                <div className="agent-review-head">
                    <span className="agent-review-title">{title}</span>
                    {display && <span className={`agent-review-state ${display.group}`}>{display.label}</span>}
                    <button className="agent-review-close" onClick={onClose} aria-label="Close">
                        <IconClose size={11} />
                    </button>
                </div>
                <div className="agent-review-branch">{worktree.branch}</div>
                {summary && <div className="agent-review-summary">{summary}</div>}
                {!evidence ? (
                    <div className="agent-review-row muted">Reading the worktree…</div>
                ) : (
                    <>
                        <div className="agent-review-row">
                            <span className="agent-review-label">Changes</span>
                            <span>
                                {evidence.changed.length === 0
                                    ? "No changes yet"
                                    : `${evidence.changed.length} file${evidence.changed.length === 1 ? "" : "s"}${evidence.uncommitted ? ` · ${evidence.uncommitted} not committed` : ""}`}
                            </span>
                        </div>
                        {evidence.changed.slice(0, SHOWN_FILES).map((path) => (
                            <button key={path} className="agent-review-file" onClick={() => requestOpenFile(joinPath(worktree.path, path))}>
                                {path}
                            </button>
                        ))}
                        {evidence.changed.length > SHOWN_FILES && (
                            <div className="agent-review-row muted">and {evidence.changed.length - SHOWN_FILES} more</div>
                        )}
                        <div className="agent-review-row">
                            <span className="agent-review-label">Tests</span>
                            {!evidence.tested ? (
                                <span className="muted">No results yet</span>
                            ) : failed ? (
                                <span className="agent-review-bad">
                                    <IconClose size={11} /> {failed} failed · {passed} passed
                                </span>
                            ) : (
                                <span className="agent-review-good">
                                    <IconCheck size={11} /> {passed} passed
                                </span>
                            )}
                        </div>
                        <div className="agent-review-row">
                            <span className="agent-review-label">Running</span>
                            {evidence.url ? (
                                <span className="agent-review-mono">{evidence.url.replace("http://", "")}</span>
                            ) : (
                                <span className="muted">Nothing running</span>
                            )}
                        </div>
                    </>
                )}
                <div className="agent-review-actions">
                    <button
                        className="agent-review-btn"
                        onClick={() => {
                            onClose();
                            cmd.revealAgent(agentId);
                        }}>
                        Show agent
                    </button>
                    {evidence?.url && (
                        <button
                            className="agent-review-btn"
                            onClick={() => {
                                onClose();
                                cmd.openUrlOnDesk(agentId, evidence.url!);
                            }}>
                            <IconExternal size={11} /> Open preview
                        </button>
                    )}
                    {failures && (
                        <button
                            className="agent-review-btn primary"
                            onClick={() => {
                                onClose();
                                sendToAgent({ agentId }, { text: failures });
                            }}>
                            Send failures to agent
                        </button>
                    )}
                </div>
            </div>
        </>
    );
}
