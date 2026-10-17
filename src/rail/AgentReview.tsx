import { useEffect, useState } from "react";
import { git } from "../api/git";
import { portsApi } from "../api/ports";
import { sendToAgent } from "../agents/sendToAgent";
import { joinPath } from "../lib/paths";
import * as cmd from "../state/commands";
import { requestOpenFile } from "../state/commands/editor";
import { agentDisplayOf } from "../state/selectors";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import type { AgentWorktree } from "../state/types";
import { detectTasksWithJava } from "../tasks/application";
import { isFilePath, isTestTask, readTestResults, type TestResults } from "../tasks/testResults";
import { IconCheck, IconClose, IconExternal } from "../ui/Icons";
import { useInbox } from "./useInbox";

const SHOWN_FILES = 6;

interface ModuleTests {
    readonly cwd: string;
    readonly results: TestResults;
}

interface Evidence {
    readonly changed: readonly string[];
    readonly uncommitted: number;
    readonly tests: readonly ModuleTests[];
    readonly url: string | null;
}

/** What an agent's worktree shows about its work: changes, test results and where its server listens. */
async function gatherEvidence(agentId: string, worktree: AgentWorktree): Promise<Evidence> {
    const base = worktree.base ?? worktree.startSha;
    const [compare, status, detected, listening] = await Promise.all([
        git.compare(worktree.path, base, "HEAD").catch(() => null),
        git.status(worktree.path).catch(() => null),
        detectTasksWithJava(worktree.path).catch(() => null),
        portsApi.listening().catch(() => []),
    ]);
    const uncommitted = status?.files.map((file) => file.path) ?? [];
    const changed = [...new Set([...(compare?.files.map((file) => file.path) ?? []), ...uncommitted])];
    const modules = [...new Set((detected?.tasks ?? []).filter((task) => isTestTask(task.id)).map((task) => task.cwd))];
    const tests = (await Promise.all(modules.map(async (cwd) => ({ cwd, results: await readTestResults(cwd).catch(() => null) })))).flatMap(
        ({ cwd, results }) => (results ? [{ cwd, results }] : []),
    );
    const port = listening.find((candidate) => candidate.owner.agentId === agentId);
    return { changed, uncommitted: uncommitted.length, tests, url: port ? `http://localhost:${port.port}` : null };
}

function failureReport(tests: readonly ModuleTests[]): string {
    const lines = tests.flatMap(({ results }) =>
        results.failures.map(
            (failure) =>
                `- ${isFilePath(failure.className) ? `${failure.className} › ` : `${failure.className}.`}${failure.name}: ${failure.message}${failure.line ? ` (line ${failure.line})` : ""}`,
        ),
    );
    return `These tests fail in your worktree:\n${lines.join("\n")}\nFix them and run the tests again.`;
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
    const display = useStore((state) => agentDisplayOf(state, agentId));
    const handoff = useInbox().inbox.items.find((item) => item.agentId === agentId)?.handoff ?? null;
    const [evidence, setEvidence] = useState<Evidence | null>(null);

    useEffect(() => {
        let cancelled = false;
        void gatherEvidence(agentId, worktree)
            .then((found) => {
                if (!cancelled) setEvidence(found);
            })
            .catch(swallow("agent review"));
        return () => {
            cancelled = true;
        };
    }, [agentId, worktree]);

    const passed = evidence?.tests.reduce((sum, { results }) => sum + results.passed, 0) ?? 0;
    const failed = evidence?.tests.reduce((sum, { results }) => sum + results.failed, 0) ?? 0;
    const summary = handoff?.result?.split("\n").find((line) => line.trim()) ?? null;
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
                    <span className={`agent-review-state ${display.group}`}>{display.label}</span>
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
                            {evidence.tests.length === 0 ? (
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
                    {evidence && failed > 0 && (
                        <button
                            className="agent-review-btn primary"
                            onClick={() => {
                                onClose();
                                sendToAgent({ agentId }, { text: failureReport(evidence.tests) });
                            }}>
                            Send failures to agent
                        </button>
                    )}
                </div>
            </div>
        </>
    );
}
