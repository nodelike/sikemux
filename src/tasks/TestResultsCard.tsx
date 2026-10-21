import { requestOpenFile } from "../state/commands/editor";
import { IconAgent, IconClose, IconRefresh, IconTerminal } from "../ui/Icons";
import type { ProjectTask, TestResults } from "./coreTasks";
import { basename } from "../lib/paths";

export interface FinishedTestRun {
    /** Changes with every run, so a card dismissed for one run opens again for the next. */
    readonly key: string;
    readonly task: ProjectTask;
    readonly results: TestResults;
}

/** `FooTest` for a Java class, `price.test.ts` for a file a JavaScript runner names. */
function shortClass(className: string): string {
    if (className.includes("/") || /\.[cm]?[jt]sx?$/.test(className)) return basename(className);
    return className.split(".").pop() ?? className;
}

export function TestResultsCard({
    run,
    onClose,
    onRerunFailed,
    onShowOutput,
    onFix,
}: {
    run: FinishedTestRun;
    onClose: () => void;
    onRerunFailed: (() => void) | null;
    onShowOutput: () => void;
    onFix: (anchor: HTMLElement) => void;
}) {
    const { results } = run;
    return (
        <>
            <div className="env-dd-scrim" onClick={onClose} />
            <div className="env-dd-menu tb-run-check tb-run-tests" role="dialog" aria-label={`${run.task.label} results`} data-overlay>
                <div className="tb-run-check-head">
                    <span>
                        <span className="tb-run-tests-failed">{results.failed} failed</span>
                        <span className="tb-run-tests-rest">
                            {" "}
                            · {results.passed} passed{results.skipped ? ` · ${results.skipped} skipped` : ""}
                        </span>
                    </span>
                    <button className="tb-run-check-close" onClick={onClose} aria-label="Close">
                        <IconClose size={11} />
                    </button>
                </div>
                <div className="tb-run-tests-list">
                    {results.failures.map((failure) => (
                        <div key={`${failure.className}#${failure.name}`} className="tb-run-check-item bad">
                            <span className="tb-run-check-mark">
                                <IconClose size={11} />
                            </span>
                            <span className="tb-run-check-text">
                                <span>
                                    {shortClass(failure.className)} › {failure.name}
                                </span>
                                {failure.message && <span className="tb-run-tests-message">{failure.message}</span>}
                                {failure.file && (
                                    <button
                                        className="tb-run-tests-link"
                                        onClick={() => {
                                            onClose();
                                            requestOpenFile(failure.file!, failure.line ? failure.line - 1 : undefined);
                                        }}>
                                        {basename(failure.file)}
                                        {failure.line ? `:${failure.line}` : ""}
                                    </button>
                                )}
                            </span>
                        </div>
                    ))}
                </div>
                <div className="tb-run-check-actions">
                    <button className="tb-run-check-btn" onClick={onShowOutput}>
                        <IconTerminal size={11} /> Show output
                    </button>
                    {onRerunFailed && (
                        <button className="tb-run-check-btn" onClick={onRerunFailed}>
                            <IconRefresh size={11} /> Rerun failed
                        </button>
                    )}
                    <button className="tb-run-check-btn primary" onClick={(event) => onFix(event.currentTarget)}>
                        <IconAgent size={11} /> Fix with an agent
                    </button>
                </div>
            </div>
        </>
    );
}

/** Why a task stopped: its exit and command, with its output and an agent one click away. */
export function FailureCard({
    task,
    summary,
    onClose,
    onShowOutput,
    onFix,
}: {
    task: ProjectTask;
    summary: string;
    onClose: () => void;
    onShowOutput: () => void;
    onFix: (anchor: HTMLElement) => void;
}) {
    return (
        <>
            <div className="env-dd-scrim" onClick={onClose} />
            <div className="env-dd-menu tb-run-check" role="dialog" aria-label={`Why ${task.label} stopped`} data-overlay>
                <div className="tb-run-check-head">
                    <span>
                        {task.label} <span className="tb-run-tests-failed">{summary}</span>
                    </span>
                    <button className="tb-run-check-close" onClick={onClose} aria-label="Close">
                        <IconClose size={11} />
                    </button>
                </div>
                <div className="tb-run-check-item">
                    <span className="tb-run-check-text">
                        <span className="tb-run-tests-message">{task.command}</span>
                        <span className="tb-run-check-detail">The reason is in its output, usually near the first error.</span>
                    </span>
                </div>
                <div className="tb-run-check-actions">
                    <button className="tb-run-check-btn" onClick={onShowOutput}>
                        <IconTerminal size={11} /> Show output
                    </button>
                    <button className="tb-run-check-btn primary" onClick={(event) => onFix(event.currentTarget)}>
                        <IconAgent size={11} /> Fix with an agent
                    </button>
                </div>
            </div>
        </>
    );
}
