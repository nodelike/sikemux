import { useMemo, useRef, useState } from "react";
import type { PluginTopBarProps } from "../../../plugin-api";
import { currentBranch, git, gitOverviewR, notify, reportError } from "../../../plugin-api/host";
import { invalidate, peekResource, useResourceEnabled } from "../../../plugin-api/resources";
import { IconCheck, IconCheckout, IconChevron, IconRundeck, Tooltip } from "../../../plugin-api/ui";
import type { MatrixCell, RundeckJob } from "../api";
import { rndJobCellsR, rndJobIndexR, rndProjectsR, useProdEnvs } from "../resources";
import { basenameOf, displayStatus, groupSegments, isLiveStatus, isProdTarget, targetTone, type EnvTone } from "../shape";
import { openRundeckJob, rundeckSettings, setDeployTarget } from "../state";
import { useRefreshEvery } from "../../../plugin-api/hooks";
import { branchKind, statusKind } from "./branchStyle";
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
    const prodEnvs = useProdEnvs();
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
    const cells = useResourceEnabled(matches.length > 0, rndJobCellsR, matches);
    const deploying = cells.data?.some((cell) => isLiveStatus(cell.latest?.status)) ?? false;
    useRefreshEvery(cells, matches.length === 0 ? null : deploying ? 10_000 : 60_000);

    const active =
        matches.find((job) => picked && job.project === picked.project && job.id === picked.jobId) ??
        matches.find((job) => !isProdTarget(job.project, job.group, prodEnvs)) ??
        matches[0] ??
        null;
    if (!projectCwd || !active) return null;
    const cell = cells.data?.find((c) => c.job_id === active.id) ?? null;

    return (
        <>
            <DeployChip
                jobs={matches}
                job={active}
                cell={cell}
                repo={projectCwd}
                prodEnvs={prodEnvs}
                onPick={(job) => setDeployTarget(projectCwd, { project: job.project, jobId: job.id })}
            />
            <span className="tb-sep" />
        </>
    );
}

/** Where the project deploys, and one menu to pick the place, deploy there, or check out what it runs. */
function DeployChip({
    jobs,
    job,
    cell,
    repo,
    prodEnvs,
    onPick,
}: {
    jobs: RundeckJob[];
    job: RundeckJob;
    cell: MatrixCell | null;
    repo: string;
    prodEnvs: string[];
    onPick: (job: RundeckJob) => void;
}) {
    const repoStatus = useResourceEnabled(true, gitOverviewR, repo);
    const checkedOut = currentBranch(repoStatus.data?.status);
    const detached = !!repoStatus.data && checkedOut === null;
    const branch = checkedOut ?? "";
    const deployed = cell?.deployed?.branch ?? null;
    const latest = cell?.latest ?? null;
    const runsBranch = !cell || deployed !== null || latest?.branch != null;
    const [checkingOut, setCheckingOut] = useState(false);
    const [menuOpen, setMenuOpen] = useState(false);
    const menuRef = useRef<HTMLDivElement>(null);
    useMenuKeys(menuOpen, menuRef, () => setMenuOpen(false));
    const dot = (candidate: RundeckJob) => DOT_CLASS[targetTone(candidate.project, candidate.group, prodEnvs)];
    const place = locationLabel(job);

    const deployBranch = () => {
        setMenuOpen(false);
        openRundeckJob(
            { project: job.project, jobId: job.id, name: job.name, group: job.group, repoPath: repo },
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

    const lastRun =
        latest && `Last run ${displayStatus(latest.status, latest.custom_status).toLowerCase()}${latest.branch ? ` · ${latest.branch}` : ""}`;
    const deployLabel = !runsBranch
        ? `Run on ${place}`
        : repoStatus.status === "loading" && !repoStatus.data
          ? "Deploy (reading the branch…)"
          : detached
            ? "Deploy (detached HEAD)"
            : "Deploy";

    return (
        <span className="tb-deploy-actions" data-no-window-drag>
            <Tooltip label={[deployed ? `${place} runs ${deployed}` : place, lastRun].filter(Boolean).join(" · ")} disabled={menuOpen}>
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
                                        key={`${candidate.project}:${candidate.id}`}
                                        className={`env-dd-item${candidate.id === job.id ? " active" : ""}`}
                                        role="menuitemradio"
                                        aria-checked={candidate.id === job.id}
                                        onClick={() => onPick(candidate)}>
                                        <span className="tree-ctx-icon">
                                            <span className={`env-dot ${dot(candidate)}`} />
                                        </span>
                                        <span className="env-dd-label">{locationLabel(candidate)}</span>
                                        {candidate.id === job.id && <IconCheck size={11} className="env-dd-check" />}
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
                        {latest && (
                            <>
                                <div className="env-dd-sep" />
                                <div className="env-dd-info">
                                    <span className="tree-ctx-icon">
                                        <i className={`tb-deploy-dot ${statusKind(latest.status)}`} aria-hidden="true" />
                                    </span>
                                    <span>{lastRun}</span>
                                </div>
                            </>
                        )}
                    </div>
                </>
            )}
        </span>
    );
}
