import { useState } from "react";
import { formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, Dropdown, EmptyState, Pending, StaleNotice } from "../../../plugin-api/ui";
import type { VercelDeployment, VercelProject } from "../api";
import { vercelDeploymentsV } from "../resources";
import { isUnderway, type EnvironmentFilter, type StatusFilter, type VercelView } from "../state";
import { Avatar, EnvBadge, IconMore, Source, Status, useDeploymentActions } from "./parts";

const ENVIRONMENTS: { value: EnvironmentFilter; label: string }[] = [
    { value: "all", label: "All Environments" },
    { value: "production", label: "Production" },
    { value: "preview", label: "Preview" },
];

const STATUSES: { value: StatusFilter; label: string }[] = [
    { value: "all", label: "All Statuses" },
    { value: "READY", label: "Ready" },
    { value: "ERROR", label: "Error" },
    { value: "BUILDING", label: "Building" },
    { value: "CANCELED", label: "Canceled" },
];

/** A project's deployments as Vercel lists them: what, how it went, where, from what, when and by whom. */
export function Deployments({
    paneId,
    active,
    team,
    project,
    find,
    environment,
    status,
    onFilter,
    onOpen,
}: {
    paneId: string;
    active: boolean;
    team: string;
    project: VercelProject;
    find: string;
    environment: EnvironmentFilter;
    status: StatusFilter;
    onFilter: (patch: Partial<Pick<VercelView, "environment" | "status">>) => void;
    onOpen: (id: string) => void;
}) {
    const deployments = useResourceEnabled(active, vercelDeploymentsV, paneId, team, project.name);
    const listed = deployments.data?.deployments;
    const actions = useDeploymentActions(team);
    const [menu, setMenu] = useState<{ deployment: VercelDeployment; x: number; y: number } | null>(null);

    const words = find.trim().toLowerCase();
    const shown = (listed ?? []).filter(
        (each) =>
            (environment === "all" || each.target === environment) &&
            (status === "all" || each.state === status || (status === "BUILDING" && isUnderway(each.state))) &&
            (!words || [each.message, each.branch, each.commit, each.author, each.url].some((field) => field?.toLowerCase().includes(words))),
    );

    return (
        <div className="vercel-deployments">
            <div className="vercel-filters">
                <Dropdown
                    className={`vercel-chip${environment !== "all" ? " on" : ""}`}
                    label="Environment"
                    value={environment}
                    options={ENVIRONMENTS}
                    onChange={(value) => onFilter({ environment: value as EnvironmentFilter })}
                />
                <Dropdown
                    className={`vercel-chip${status !== "all" ? " on" : ""}`}
                    label="Status"
                    value={status}
                    options={STATUSES}
                    onChange={(value) => onFilter({ status: value as StatusFilter })}
                />
            </div>
            <StaleNotice handle={deployments} />
            {!listed ? (
                <Pending handle={deployments} loading="Loading deployments" />
            ) : shown.length === 0 ? (
                <EmptyState message={listed.length === 0 ? "No deployments yet." : "No deployment matches these filters."} />
            ) : (
                <div className="vercel-table" role="table" aria-label="Deployments">
                    {shown.map((each) => {
                        const current = each.id === project.production?.id;
                        return (
                            <div key={each.id} className="vercel-table-row" role="row">
                                <button
                                    type="button"
                                    className="vercel-table-open"
                                    onClick={() => onOpen(each.id)}
                                    aria-label={`Open ${each.message || each.id}`}>
                                    <span className="vercel-cell title">{each.message || each.url?.replace(/^https:\/\//, "") || each.id}</span>
                                    <span className="vercel-cell status">
                                        <Status deployment={each} duration={formatDuration} />
                                    </span>
                                    <span className="vercel-cell env">
                                        <EnvBadge deployment={each} current={current} />
                                    </span>
                                    <span className="vercel-cell source">
                                        <Source deployment={each} />
                                    </span>
                                    <span className="vercel-cell when">
                                        <span className="vercel-muted">{formatAgo(each.createdAt)}</span>
                                        <Avatar name={each.author} />
                                    </span>
                                </button>
                                <button
                                    type="button"
                                    className="vercel-button icon ghost"
                                    aria-label={`Actions for ${each.message || each.id}`}
                                    onClick={(event) => setMenu({ deployment: each, x: event.clientX, y: event.clientY })}>
                                    <IconMore />
                                </button>
                            </div>
                        );
                    })}
                </div>
            )}
            {menu && (
                <ContextMenu
                    x={menu.x}
                    y={menu.y}
                    items={actions.menu(menu.deployment, menu.deployment.id === project.production?.id)}
                    onClose={() => setMenu(null)}
                />
            )}
        </div>
    );
}
