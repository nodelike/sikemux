import { useState } from "react";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { invalidate } from "../../plugin-api/resources";
import { Tooltip } from "../../plugin-api/ui";
import { hostApi, type RepoRef, type Run } from "../api";
import { useHost } from "../registry";
import { isUnfinished } from "../runStatus";
import { MoreDots } from "./ActionsIcon";

interface Props {
    run: Run;
    repo: RepoRef;
    canWrite: boolean;
    onDeleted: () => void;
}

export function RunMenu({ run, repo, canWrite, onDeleted }: Props) {
    const host = useHost();
    const { ci } = host.capabilities;
    const [open, setOpen] = useState(false);
    const finished = !isUnfinished(run);

    const act = (work: () => Promise<void>) => {
        setOpen(false);
        void work();
    };

    const rerunWithDebug = () =>
        hostApi(repo.provider)
            .rerun(repo, run.id, false, true)
            .then(() => {
                notify("success", "Re-running everything with debug logs");
                invalidate((kind) => kind === "host.runs" || kind === "host.run");
            })
            .catch(reportError("Could not re-run it"));

    const deleteLogs = async () => {
        const sure = await confirmDialog({
            title: "Delete this run's logs?",
            body: `Every job log from run #${run.runNumber} goes for good. The run itself stays.`,
            confirmLabel: "Delete logs",
            destructive: true,
        });
        if (!sure) return;
        await hostApi(repo.provider)
            .deleteRunLogs(repo, run.id)
            .then(() => {
                notify("success", "Deleted the logs");
                invalidate((kind) => kind === "host.jobLog");
            })
            .catch(reportError("Could not delete the logs"));
    };

    const deleteRun = async () => {
        const sure = await confirmDialog({
            title: "Delete this run?",
            body: `Run #${run.runNumber}, its logs and its artifacts go for good.`,
            confirmLabel: "Delete run",
            destructive: true,
        });
        if (!sure) return;
        await hostApi(repo.provider)
            .deleteRun(repo, run.id)
            .then(() => {
                notify("success", `Deleted run #${run.runNumber}`);
                onDeleted();
                invalidate((kind) => kind === "host.runs");
            })
            .catch(reportError("Could not delete the run"));
    };

    return (
        <span className="gha-menu-anchor">
            <Tooltip label="More">
                <button
                    type="button"
                    className="gha-icon-btn"
                    aria-label="More run actions"
                    aria-haspopup="menu"
                    aria-expanded={open}
                    onClick={() => setOpen((was) => !was)}>
                    <MoreDots size={13} />
                </button>
            </Tooltip>
            {open && (
                <>
                    <div className="env-dd-scrim" onClick={() => setOpen(false)} />
                    <div
                        className="env-dd-menu gha-menu"
                        role="menu"
                        aria-label="Run actions"
                        onKeyDown={(event) => event.key === "Escape" && setOpen(false)}>
                        {canWrite && ci.debugLogs && finished && (
                            <button type="button" className="env-dd-item" role="menuitem" onClick={() => act(rerunWithDebug)}>
                                Re-run all with debug logs
                            </button>
                        )}
                        <button
                            type="button"
                            className="env-dd-item"
                            role="menuitem"
                            onClick={() =>
                                act(() =>
                                    copyText(run.url)
                                        .then(() => notify("success", "Copied the link"))
                                        .catch(swallow("copy the link")),
                                )
                            }>
                            Copy link
                        </button>
                        <button
                            type="button"
                            className="env-dd-item"
                            role="menuitem"
                            onClick={() => act(() => openUrl(run.url).catch(swallow(`open ${host.name}`)))}>
                            Open on {host.name}
                        </button>
                        {canWrite && ci.deleteRuns && finished && (
                            <>
                                <button type="button" className="env-dd-item danger" role="menuitem" onClick={() => act(deleteLogs)}>
                                    Delete all logs
                                </button>
                                <button type="button" className="env-dd-item danger" role="menuitem" onClick={() => act(deleteRun)}>
                                    Delete run
                                </button>
                            </>
                        )}
                    </div>
                </>
            )}
        </span>
    );
}
