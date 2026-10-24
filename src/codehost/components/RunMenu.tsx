import { useState, type ReactNode } from "react";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { IconCopy, IconExternal, IconRotate, IconTrash, Tooltip } from "../../plugin-api/ui";
import { hostApi, type RepoRef, type Run } from "../api";
import { useHost } from "../registry";
import { MoreDots } from "./ActionsIcon";

interface Props {
    run: Run;
    /** As the core says: still going, or held for approval. */
    unfinished: boolean;
    repo: RepoRef;
    canWrite: boolean;
    onDeleted: () => void;
}

export function RunMenu({ run, unfinished, repo, canWrite, onDeleted }: Props) {
    const host = useHost();
    const { ci } = host.capabilities;
    const [open, setOpen] = useState(false);
    const finished = !unfinished;

    const act = (work: () => Promise<void>) => {
        setOpen(false);
        void work();
    };

    const rerunWithDebug = () =>
        hostApi(repo.provider)
            .rerun(repo, run.id, false, true)
            .then(() => notify("success", "Re-running everything with debug logs"))
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
            .then(() => notify("success", "Deleted the logs"))
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
            })
            .catch(reportError("Could not delete the run"));
    };

    const deletable = canWrite && finished;
    const items: { label: string; icon: ReactNode; danger?: boolean; run: () => Promise<void> }[] = [
        ...(canWrite && ci.debugLogs && finished
            ? [{ label: "Re-run all with debug logs", icon: <IconRotate size={13} />, run: rerunWithDebug }]
            : []),
        {
            label: "Copy link",
            icon: <IconCopy size={13} />,
            run: () =>
                copyText(run.url)
                    .then(() => notify("success", "Copied the link"))
                    .catch(swallow("copy the link")),
        },
        { label: `Open on ${host.name}`, icon: <IconExternal size={13} />, run: () => openUrl(run.url).catch(swallow(`open ${host.name}`)) },
        ...(deletable && ci.deleteLogs ? [{ label: "Delete all logs", icon: <IconTrash size={13} />, danger: true, run: deleteLogs }] : []),
        ...(deletable && ci.deleteRuns ? [{ label: "Delete run", icon: <IconTrash size={13} />, danger: true, run: deleteRun }] : []),
    ];

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
                        {items.map((item) => (
                            <button
                                key={item.label}
                                type="button"
                                className={`env-dd-item${item.danger ? " danger" : ""}`}
                                role="menuitem"
                                onClick={() => act(item.run)}>
                                <span className="tree-ctx-icon">{item.icon}</span>
                                {item.label}
                            </button>
                        ))}
                    </div>
                </>
            )}
        </span>
    );
}
