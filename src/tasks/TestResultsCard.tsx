import { useEffect, useState } from "react";
import { requestOpenFile } from "../state/commands/editor";
import { swallow } from "../state/toast";
import { IconClose, IconRefresh, IconTerminal } from "../ui/Icons";
import type { ResolvedTaskDefinition, TaskControllerSnapshot } from "./taskRegistry";
import { isTestTask, readTestResults, type TestResults } from "./testResults";

export interface FinishedTestRun {
    /** Changes with every run, so a card dismissed for one run opens again for the next. */
    readonly key: string;
    readonly task: ResolvedTaskDefinition;
    readonly results: TestResults;
}

/** The JUnit results of the project's last test task, read once it has finished. */
export function useFinishedTestRun(runtime: TaskControllerSnapshot | null): FinishedTestRun | null {
    const task = runtime && runtime.activeRunId === null && runtime.task && isTestTask(runtime.task.id) ? runtime.task : null;
    const key = task && runtime ? `${task.project}\0${task.id}\0${runtime.runAttempts}` : null;
    const [found, setFound] = useState<FinishedTestRun | null>(null);
    useEffect(() => {
        if (!task || !key) return;
        let cancelled = false;
        void readTestResults(task.cwd)
            .then((results) => {
                if (!cancelled && results) setFound({ key, task, results });
            })
            .catch(swallow("read test results"));
        return () => {
            cancelled = true;
        };
        // The key names the run; the task object is rebuilt on every registry change.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [key]);
    return found && found.key === key ? found : null;
}

/** `FooTest` for a Java class, `price.test.ts` for a file a JavaScript runner names. */
function shortClass(className: string): string {
    if (className.includes("/") || /\.[cm]?[jt]sx?$/.test(className)) return className.split("/").pop() ?? className;
    return className.split(".").pop() ?? className;
}

export function TestResultsCard({
    run,
    onClose,
    onRerunFailed,
    onShowOutput,
}: {
    run: FinishedTestRun;
    onClose: () => void;
    onRerunFailed: (() => void) | null;
    onShowOutput: () => void;
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
                                        {failure.file.split("/").pop()}
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
                        <button className="tb-run-check-btn primary" onClick={onRerunFailed}>
                            <IconRefresh size={11} /> Rerun failed
                        </button>
                    )}
                </div>
            </div>
        </>
    );
}
