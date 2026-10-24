import { useRef, useState } from "react";
import type { PluginTopBarProps } from "../../../plugin-api";
import { currentBranch, git, gitOverviewR, notify, reportError } from "../../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../../plugin-api/resources";
import { IconCheck, IconCheckout, IconChevron, IconRundeck, Tooltip } from "../../../plugin-api/ui";
import { rndDeployTargetV } from "../resources";
import { basenameOf } from "../shape";
import { markRundeckLooked, openRundeckJob, rundeckLooked, rundeckSettings, setDeployTarget } from "../state";
import type { RundeckDeployTarget, RundeckEnvTone, RundeckJobPlace } from "../types.generated";
import { branchKind } from "./branchStyle";
import { useMenuKeys } from "./hooks";
import "../branch.css";

const DOT_CLASS: Record<RundeckEnvTone, string> = { prod: "production", staging: "staging", preprod: "preprod", dev: "dev", other: "other" };

/**
 * Where the project in front is deployed, and what is running there.
 *
 * Finding it asks a deploy server that most projects have nothing to do with,
 * so it waits until something else has already asked Rundeck for its jobs, or
 * the pointer moves over the strip it would show in.
 */
export function RundeckTopBarItem({ projectCwd, stripHovered }: PluginTopBarProps) {
    const folder = projectCwd ? basenameOf(projectCwd) : null;
    const picked = rundeckSettings.useSelect((s) => (projectCwd ? s.deployTargets[projectCwd] : undefined));
    if (stripHovered) markRundeckLooked();
    const target = useResourceEnabled(!!folder && rundeckLooked(), rndDeployTargetV, `topbar/${projectCwd}`, folder ?? "", picked ?? null);
    const active = target.data?.active;
    if (!projectCwd || !target.data || !active) return null;

    return (
        <>
            <DeployChip
                target={target.data}
                job={active}
                repo={projectCwd}
                onPick={(job) => setDeployTarget(projectCwd, { project: job.project, jobId: job.jobId })}
            />
            <span className="tb-sep" />
        </>
    );
}

/** Where the project deploys, and one menu to pick the place, deploy there, or check out what it runs. */
function DeployChip({
    target,
    job,
    repo,
    onPick,
}: {
    target: RundeckDeployTarget;
    job: RundeckJobPlace;
    repo: string;
    onPick: (job: RundeckJobPlace) => void;
}) {
    const jobs = target.jobs;
    const repoStatus = useResourceEnabled(true, gitOverviewR, repo);
    const checkedOut = currentBranch(repoStatus.data?.status);
    const detached = !!repoStatus.data && checkedOut === null;
    const branch = checkedOut ?? "";
    const deployed = target.deployedBranch;
    const lastRun = target.lastRun;
    const runsBranch = target.runsBranch;
    const [checkingOut, setCheckingOut] = useState(false);
    const [menuOpen, setMenuOpen] = useState(false);
    const menuRef = useRef<HTMLDivElement>(null);
    useMenuKeys(menuOpen, menuRef, () => setMenuOpen(false));
    const dot = (candidate: RundeckJobPlace) => DOT_CLASS[candidate.tone];
    const place = job.place;

    const deployBranch = () => {
        setMenuOpen(false);
        openRundeckJob(
            { project: job.project, jobId: job.jobId, name: job.name, group: job.group, repoPath: repo },
            { branch: runsBranch ? branch : undefined },
        );
    };
    const checkoutBranch = () => {
        if (!deployed || checkingOut) return;
        setCheckingOut(true);
        void git
            .checkoutSmart(repo, deployed)
            .then((msg) => {
                notify("success", msg);
                invalidate((kind, args) => (kind.startsWith("git.") || kind === "files.list") && args[0] === repo);
                setMenuOpen(false);
            })
            .catch(reportError("checkout deployed branch"))
            .finally(() => setCheckingOut(false));
    };

    const deployLabel = !runsBranch
        ? `Run on ${place}`
        : repoStatus.status === "loading" && !repoStatus.data
          ? "Deploy (reading the branch…)"
          : detached
            ? "Deploy (detached HEAD)"
            : "Deploy";

    return (
        <span className="tb-deploy-actions" data-no-window-drag>
            <Tooltip label={target.summary} disabled={menuOpen}>
                <button className="tb-deploy-chip" onClick={() => setMenuOpen((v) => !v)} aria-haspopup="menu" aria-expanded={menuOpen}>
                    <IconRundeck size={12} />
                    <span className={`env-dot ${dot(job)}`} />
                    <span className="tb-deploy-place">{place}</span>
                    <IconChevron size={10} className="env-dd-chev" />
                </button>
            </Tooltip>
            {menuOpen && (
                <>
                    <div className="env-dd-scrim" onClick={() => setMenuOpen(false)} />
                    <div className="env-dd-menu tb-deploy-menu" role="menu" aria-label="Deploy" ref={menuRef}>
                        {jobs.length > 1 && (
                            <>
                                <div className="env-dd-group">Deploy to</div>
                                {jobs.map((candidate) => (
                                    <button
                                        key={`${candidate.project}:${candidate.jobId}`}
                                        className={`env-dd-item${candidate.jobId === job.jobId ? " active" : ""}`}
                                        role="menuitemradio"
                                        aria-checked={candidate.jobId === job.jobId}
                                        onClick={() => onPick(candidate)}>
                                        <span className="tree-ctx-icon">
                                            <span className={`env-dot ${dot(candidate)}`} />
                                        </span>
                                        <span className="env-dd-label">{candidate.place}</span>
                                        {candidate.jobId === job.jobId && <IconCheck size={11} className="env-dd-check" />}
                                    </button>
                                ))}
                                <div className="env-dd-sep" />
                            </>
                        )}
                        <button
                            className="env-dd-item"
                            role="menuitem"
                            onClick={deployBranch}
                            disabled={runsBranch && repoStatus.status === "loading" && !repoStatus.data}>
                            <span className="tree-ctx-icon">
                                <IconRundeck size={13} />
                            </span>
                            <span>{deployLabel}</span>
                            {runsBranch && branch && <span className={`tb-deploy-menu-branch rnd-branch-${branchKind(branch)}`}>{branch}</span>}
                        </button>
                        {deployed && (
                            <button className="env-dd-item" role="menuitem" onClick={checkoutBranch} disabled={checkingOut || deployed === branch}>
                                <span className="tree-ctx-icon">
                                    <IconCheckout size={13} />
                                </span>
                                <span>{checkingOut ? "Checking out…" : "Check out"}</span>
                                <span className={`tb-deploy-menu-branch rnd-branch-${branchKind(deployed)}`}>{deployed}</span>
                            </button>
                        )}
                        {lastRun && (
                            <>
                                <div className="env-dd-sep" />
                                <div className="env-dd-info">
                                    <span className="tree-ctx-icon">
                                        <i className={`tb-deploy-dot ${lastRun.statusKind}`} aria-hidden="true" />
                                    </span>
                                    <span>{lastRun.text}</span>
                                </div>
                            </>
                        )}
                    </div>
                </>
            )}
        </span>
    );
}
