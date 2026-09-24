import { useMemo, useRef, useState } from "react";
import type { PluginTopBarProps } from "../../../plugin-api";
import { git, gitOverviewR, notify, reportError } from "../../../plugin-api/host";
import { invalidate, peekResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconChevron, IconGit, IconRundeck, Tooltip } from "../../../plugin-api/ui";
import type { MatrixCell, RundeckJob } from "../api";
import { rndJobCellsR, rndJobIndexR, rndProjectsR } from "../resources";
import { basenameOf, displayStatus, groupSegments, isProdTarget, targetTone, type EnvTone } from "../shape";
import { openRundeckJob, rundeckSettings, setDeployTarget } from "../state";
import { branchKind } from "./branchStyle";
import { useMenuKeys } from "./hooks";
import "../branch.css";

const MAX_MATCHES = 12;

const DOT_CLASS: Record<EnvTone, string> = { prod: "production", staging: "staging", preprod: "preprod", dev: "dev", other: "other" };

function locationLabel(job: RundeckJob): string {
    return [job.project, ...groupSegments(job.group), job.name].join(" / ");
}

/** Jobs named after the folder: exact name first, any case only when nothing matches exactly. */
function matchJobs(jobs: RundeckJob[], folder: string): RundeckJob[] {
    const exact = jobs.filter((job) => job.name === folder);
    const found = exact.length ? exact : jobs.filter((job) => job.name.toLowerCase() === folder.toLowerCase());
    return found.sort((a, b) => locationLabel(a).localeCompare(locationLabel(b))).slice(0, MAX_MATCHES);
}

/**
 * Where the project in front is deployed, and what is running there.
 *
 * Finding it asks a deploy server that most projects have nothing to do with,
 * so it waits until something else has already asked Rundeck for its jobs, or
 * the pointer moves over the strip it would show in.
 */
export function RundeckTopBarItem({ projectCwd, stripHovered }: PluginTopBarProps) {
    const folder = projectCwd ? basenameOf(projectCwd) : null;
    const branchOptions = rundeckSettings.useSelect((s) => s.branchOptions);
    const prodEnvs = rundeckSettings.useSelect((s) => s.prodEnvs);
    const picked = rundeckSettings.useSelect((s) => (projectCwd ? s.deployTargets[projectCwd] : undefined));
    const deployWanted = stripHovered || peekResource(rndJobIndexR) !== undefined || peekResource(rndProjectsR) !== undefined;
    const index = useResourceEnabled(!!folder && deployWanted, rndJobIndexR);

    const matches = useMemo(
        () =>
            folder
                ? matchJobs(
                      (index.data ?? []).flatMap((entry) => entry.jobs),
                      folder,
                  )
                : [],
        [index.data, folder],
    );
    const cells = useResourceEnabled(matches.length > 0, rndJobCellsR, matches, branchOptions);

    const active =
        matches.find((job) => picked && job.project === picked.project && job.id === picked.jobId) ??
        matches.find((job) => !isProdTarget(job.project, job.group, prodEnvs)) ??
        matches[0] ??
        null;
    if (!projectCwd || !active) return null;
    const cell = cells.data?.find((c) => c.job_id === active.id) ?? null;

    return (
        <>
            <LocationPicker
                jobs={matches}
                active={active}
                prodEnvs={prodEnvs}
                onPick={(job) => setDeployTarget(projectCwd, { project: job.project, jobId: job.id })}
            />
            <DeployChip job={active} cell={cell} repo={projectCwd} />
            <span className="tb-sep" />
        </>
    );
}

function LocationPicker({
    jobs,
    active,
    prodEnvs,
    onPick,
}: {
    jobs: RundeckJob[];
    active: RundeckJob;
    prodEnvs: string[];
    onPick: (job: RundeckJob) => void;
}) {
    const [open, setOpen] = useState(false);
    const menuRef = useRef<HTMLDivElement>(null);
    useMenuKeys(open, menuRef, () => setOpen(false));
    const many = jobs.length > 1;
    const dot = (job: RundeckJob) => DOT_CLASS[targetTone(job.project, job.group, prodEnvs)];

    return (
        <div className="env-dd" data-no-window-drag>
            <button
                className="env-dd-btn"
                type="button"
                aria-haspopup="listbox"
                aria-expanded={open}
                onClick={() => many && setOpen((v) => !v)}
                aria-label={many ? "Switch deploy location" : locationLabel(active)}>
                <span className={`env-dot ${dot(active)}`} />
                <span className="env-dd-label">{locationLabel(active)}</span>
                {many && <IconChevron size={10} className="env-dd-chev" />}
            </button>
            {open && many && (
                <>
                    <div className="env-dd-scrim" onClick={() => setOpen(false)} />
                    <div className="env-dd-menu" role="listbox" aria-label="Deploy location" ref={menuRef}>
                        {jobs.map((job) => (
                            <button
                                key={`${job.project}:${job.id}`}
                                className={`env-dd-item${job.id === active.id ? " active" : ""}`}
                                role="option"
                                aria-selected={job.id === active.id}
                                onClick={() => {
                                    onPick(job);
                                    setOpen(false);
                                }}>
                                <span className={`env-dot ${dot(job)}`} />
                                <span>{locationLabel(job)}</span>
                            </button>
                        ))}
                    </div>
                </>
            )}
        </div>
    );
}

function DeployChip({ job, cell, repo }: { job: RundeckJob; cell: MatrixCell | null; repo: string }) {
    const repoStatus = useResourceEnabled(true, gitOverviewR, repo);
    const head = repoStatus.data?.status.branch.trim() ?? "";
    const detached = head === "HEAD";
    const currentBranch = detached ? "" : head;
    const deployed = cell?.deployed?.branch ?? null;
    const latest = cell?.latest ?? null;
    const runsBranch = !cell || deployed !== null || latest?.branch != null;
    const [checkingOut, setCheckingOut] = useState(false);
    const [menuOpen, setMenuOpen] = useState(false);
    const menuRef = useRef<HTMLDivElement>(null);
    useMenuKeys(menuOpen, menuRef, () => setMenuOpen(false));

    const deployBranch = () => {
        setMenuOpen(false);
        openRundeckJob(
            { project: job.project, jobId: job.id, name: job.name, group: job.group, repoPath: repo },
            { branch: runsBranch ? currentBranch : undefined },
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

    const summary = latest
        ? `${displayStatus(latest.status, latest.custom_status)}${latest.branch ? ` · ${latest.branch}` : ""} · ${locationLabel(job)}`
        : `Rundeck actions for ${locationLabel(job)}`;
    const deployLabel = !runsBranch
        ? "run"
        : repoStatus.status === "loading" && !head
          ? "deploy (loading branch…)"
          : detached
            ? "deploy (detached HEAD)"
            : "deploy";

    return (
        <span className="tb-deploy-actions" data-no-window-drag>
            <Tooltip label={deployed ? `deployed: ${deployed} · last run: ${summary}` : summary}>
                <button className="tb-deploy-chip" onClick={() => setMenuOpen((v) => !v)} aria-haspopup="menu" aria-expanded={menuOpen}>
                    <IconRundeck size={12} />
                    {deployed && <span className={`tb-deploy-branch rnd-branch-${branchKind(deployed)}`}>{deployed}</span>}
                    <IconChevron size={10} className="env-dd-chev" />
                </button>
            </Tooltip>
            {menuOpen && (
                <>
                    <div className="env-dd-scrim" onClick={() => setMenuOpen(false)} />
                    <div className="env-dd-menu tb-deploy-menu" role="menu" aria-label="Rundeck actions" ref={menuRef}>
                        <button
                            className="env-dd-item"
                            role="menuitem"
                            onClick={deployBranch}
                            disabled={runsBranch && repoStatus.status === "loading" && !head}>
                            <IconRundeck size={12} />
                            <span>{deployLabel}</span>
                            {runsBranch && currentBranch && (
                                <span className={`tb-deploy-menu-branch rnd-branch-${branchKind(currentBranch)}`}>{currentBranch}</span>
                            )}
                        </button>
                        {deployed && (
                            <Tooltip
                                side="left"
                                label={`Checkout deployed branch ${deployed}. If it only exists on a remote, Sikemux will fetch and create a tracking local branch.`}>
                                <button
                                    className="env-dd-item"
                                    role="menuitem"
                                    onClick={checkoutBranch}
                                    disabled={checkingOut || deployed === currentBranch}>
                                    <IconGit size={12} />
                                    <span>{checkingOut ? "checking out…" : "checkout"}</span>
                                </button>
                            </Tooltip>
                        )}
                    </div>
                </>
            )}
        </span>
    );
}
