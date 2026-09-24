import { type RundeckStatus } from "../api";
import * as cmd from "../state";
import type { JobRef, RundeckLevel } from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconChevron, IconPanelLeft, Tooltip } from "../../../plugin-api/ui";
import { rndJobDetailR } from "../resources";
import { branchOptionName, groupSegments } from "../shape";
import { RundeckSettingsPopover } from "./RundeckSettingsPopover";

interface Props {
    paneId: string;
    status: RundeckStatus | null;
    signedIn: boolean;
    onSignedOut: () => void;
}

interface Crumb {
    key: string;
    label: string;
    onClick?: () => void;
}

export function RundeckBreadcrumb({ paneId, status, signedIn, onSignedOut }: Props) {
    const { stack } = cmd.useRundeckView(paneId);
    const activeProject = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const activeGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const treeHidden = cmd.rundeckSettings.useSelect((s) => s.treeHidden);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const top = stack[stack.length - 1];
    const deployJobId = top?.kind === "deploy" ? top.jobId : "";
    const detail = useResourceEnabled(!!deployJobId, rndJobDetailR, deployJobId);
    const runsBranch = detail.data
        ? branchOptionName(
              detail.data.options.map((o) => o.name),
              branchOptions,
          ) !== null
        : !!detail.error;

    const crumbs = breadcrumbs(paneId, stack, activeProject, activeGroup, runsBranch ? "deploy" : "run");

    return (
        <div className="rnd-bar">
            <button className="rnd-bar-back" aria-label="Back" disabled={stack.length <= 1} onClick={() => cmd.rundeckPop(paneId)}>
                <IconChevron size={11} className="rnd-bar-back-ic" />
            </button>
            {signedIn && (
                <Tooltip label={treeHidden ? "Show project tree" : "Hide project tree"}>
                    <button
                        className={`rnd-bar-icon${treeHidden ? "" : " on"}`}
                        aria-pressed={!treeHidden}
                        aria-label="Project tree"
                        onClick={() => cmd.updateRundeckSettings({ treeHidden: !treeHidden })}>
                        <IconPanelLeft size={13} />
                    </button>
                </Tooltip>
            )}
            <nav className="rnd-bar-trail" aria-label="Rundeck location">
                {crumbs.map((crumb, idx) => {
                    const current = idx === crumbs.length - 1;
                    return (
                        <span key={crumb.key} className="rnd-crumb-row">
                            {idx > 0 && (
                                <span className="rnd-crumb-sep" aria-hidden="true">
                                    <IconChevron size={9} />
                                </span>
                            )}
                            <button
                                className={`rnd-crumb${current ? " current" : ""}`}
                                onClick={crumb.onClick}
                                disabled={current || !crumb.onClick}
                                aria-current={current ? "page" : undefined}>
                                {crumb.label}
                            </button>
                        </span>
                    );
                })}
            </nav>
            <div className="rnd-bar-right">
                {status?.url && (
                    <span
                        className="rnd-host"
                        title={[`${status.user ?? ""}@${status.url}`, status.ok ? status.message : null].filter(Boolean).join(" · ")}>
                        {hostFromUrl(status.url)}
                    </span>
                )}
                {signedIn && <RundeckSettingsPopover onSignedOut={onSignedOut} />}
            </div>
        </div>
    );
}

function breadcrumbs(paneId: string, stack: RundeckLevel[], activeProject: string, activeGroup: string | null, runWord: string): Crumb[] {
    const top = stack[stack.length - 1];
    if (!top || top.kind === "matrix") return locationCrumbs(paneId, activeProject, activeGroup);

    const serviceIndex = findPriorServiceIndex(stack, top.jobId);
    const job: Crumb = {
        key: `job-${top.jobId}`,
        label: top.name,
        onClick: serviceIndex >= 0 ? () => cmd.rundeckPopTo(paneId, serviceIndex) : undefined,
    };
    const base = [...locationCrumbs(paneId, top.project, top.group), job];
    if (top.kind === "service") return base;
    if (top.kind === "deploy") return [...base, { key: "run", label: runWord }];
    return [...base, { key: `execution-${top.executionId}`, label: `#${top.executionId}` }];
}

function locationCrumbs(paneId: string, project: string, group: string | null): Crumb[] {
    if (!project) return [{ key: "deployments", label: "deployments", onClick: () => cmd.rundeckHome(paneId) }];
    const segments = groupSegments(group);
    return [
        { key: `project-${project}`, label: project, onClick: () => cmd.selectRundeckGroup(paneId, project, null) },
        ...segments.map((segment, index) => {
            const path = segments.slice(0, index + 1).join("/");
            return { key: `group-${path}`, label: segment, onClick: () => cmd.selectRundeckGroup(paneId, project, path) };
        }),
    ];
}

function findPriorServiceIndex(stack: RundeckLevel[], jobId: JobRef["jobId"]): number {
    for (let i = stack.length - 2; i >= 0; i -= 1) {
        const level = stack[i];
        if (level.kind === "service" && level.jobId === jobId) return i;
    }
    return -1;
}

function hostFromUrl(url: string): string {
    try {
        return new URL(url).host;
    } catch {
        return url;
    }
}
