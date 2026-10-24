import { formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, EmptyState } from "../../../plugin-api/ui";
import { useState } from "react";
import type { VercelProject } from "../api";
import { vercelDeploymentsV, vercelDomainsV } from "../resources";
import {
    Avatar,
    EnvBadge,
    hostOf,
    IconExternal,
    IconGitHub,
    IconMore,
    IconRollback,
    Source,
    StateDot,
    Status,
    stateLabel,
    useDeploymentActions,
    visit,
} from "./parts";

/** A project's front page, as Vercel's: the deployment serving production, and each branch's latest preview. */
export function Overview({
    paneId,
    active,
    team,
    scopeSlug,
    project,
    onOpen,
    onDeployments,
}: {
    paneId: string;
    active: boolean;
    team: string;
    scopeSlug: string;
    project: VercelProject;
    onOpen: (id: string) => void;
    onDeployments: () => void;
}) {
    const deployments = useResourceEnabled(active, vercelDeploymentsV, paneId, team, project.name).data?.deployments;
    const domains = useResourceEnabled(active, vercelDomainsV, paneId, team, project.name).data?.domains;
    const actions = useDeploymentActions(team);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const production = project.production;
    const current = deployments?.find((each) => each.id === production?.id);
    const previous = deployments?.find((each) => each.target === "production" && each.state === "READY" && each.id !== production?.id);
    const branches = latestPerBranch((deployments ?? []).filter((each) => each.target === "preview"));

    return (
        <div className="vercel-overview">
            <section className="vercel-box" aria-label="Production deployment">
                <header className="vercel-box-head">
                    <span className="vercel-box-title">Production Deployment</span>
                    <span className="vercel-actions">
                        {project.repo && project.gitProvider === "github" && (
                            <button
                                type="button"
                                className="vercel-button icon"
                                aria-label="Open the repository"
                                onClick={() => visit(`https://github.com/${project.repo}`)}>
                                <IconGitHub />
                            </button>
                        )}
                        <button
                            type="button"
                            className="vercel-button"
                            disabled={!previous || actions.busy}
                            onClick={() => previous && void actions.rollback(previous)}>
                            <IconRollback />
                            Instant Rollback
                        </button>
                        {production?.url && (
                            <button type="button" className="vercel-button primary" onClick={() => visit(production.url!)}>
                                Visit
                            </button>
                        )}
                        {current && (
                            <button
                                type="button"
                                className="vercel-button icon"
                                aria-label="More actions"
                                onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                                <IconMore />
                            </button>
                        )}
                    </span>
                </header>
                {project.latestProduction && (
                    <button type="button" className="vercel-box-note" onClick={() => onOpen(project.latestProduction!.id)}>
                        <StateDot state={project.latestProduction.state} />
                        The latest production build is {stateLabel(project.latestProduction.state).toLowerCase()} (
                        {formatAgo(project.latestProduction.createdAt)}); the deployment below still serves.
                    </button>
                )}
                {production ? (
                    <div className="vercel-production">
                        <button
                            type="button"
                            className="vercel-preview"
                            onClick={() => onOpen(production.id)}
                            aria-label="Open the production deployment">
                            <span className="vercel-preview-bar">
                                <i />
                                <i />
                                <i />
                                <span>{hostOf(production.url)}</span>
                            </span>
                            <span className="vercel-preview-body">
                                <span className="vercel-preview-name">{project.name}</span>
                                <span className="vercel-muted">{project.framework ?? "Static"}</span>
                            </span>
                        </button>
                        <dl className="vercel-facts">
                            <div>
                                <dt>Deployment</dt>
                                <dd>
                                    <button type="button" className="vercel-link" onClick={() => onOpen(production.id)}>
                                        {hostOf(current?.url ?? null) ?? production.id}
                                    </button>
                                </dd>
                            </div>
                            <div>
                                <dt>Domains</dt>
                                <dd className="vercel-domain-list">
                                    {(domains ?? [])
                                        .filter((domain) => !domain.redirect && !domain.gitBranch)
                                        .map((domain) => (
                                            <button
                                                key={domain.name}
                                                type="button"
                                                className="vercel-link"
                                                onClick={() => visit(`https://${domain.name}`)}>
                                                {domain.name}
                                                <IconExternal />
                                            </button>
                                        ))}
                                    {domains?.length === 0 && <span className="vercel-muted">None</span>}
                                </dd>
                            </div>
                            <div className="vercel-facts-pair">
                                <div>
                                    <dt>Status</dt>
                                    <dd>
                                        <Status
                                            deployment={{ state: production.state, buildMs: current?.buildMs ?? null }}
                                            duration={formatDuration}
                                        />
                                    </dd>
                                </div>
                                <div>
                                    <dt>Created</dt>
                                    <dd>
                                        {formatAgo(production.createdAt)}
                                        {current?.author && (
                                            <>
                                                {" "}
                                                by {current.author} <Avatar name={current.author} />
                                            </>
                                        )}
                                    </dd>
                                </div>
                            </div>
                            <div>
                                <dt>Source</dt>
                                <dd className="vercel-source-block">
                                    {current ? (
                                        <>
                                            <Source deployment={current} />
                                            {current.message && <span>{current.message}</span>}
                                        </>
                                    ) : (
                                        <span className="vercel-muted">—</span>
                                    )}
                                </dd>
                            </div>
                        </dl>
                    </div>
                ) : (
                    <EmptyState message="Nothing is in production yet." />
                )}
                <footer className="vercel-box-foot">
                    <span>
                        {project.productionBranch ? (
                            <>
                                To update your Production Deployment, push to the <span className="vercel-mono">{project.productionBranch}</span>{" "}
                                branch.
                            </>
                        ) : (
                            "Deploy from the Vercel CLI or a connected repository to update production."
                        )}
                    </span>
                    <span className="vercel-actions">
                        <button type="button" className="vercel-button" onClick={onDeployments}>
                            Deployments
                        </button>
                        {scopeSlug && (
                            <button type="button" className="vercel-button" onClick={() => visit(`https://vercel.com/${scopeSlug}/${project.name}`)}>
                                Open in Vercel
                                <IconExternal />
                            </button>
                        )}
                    </span>
                </footer>
            </section>

            <h2 className="vercel-heading">Active Branches</h2>
            <section className="vercel-box">
                {branches.length === 0 ? (
                    <div className="vercel-empty">
                        <b>No Active Branches</b>
                        <span>Commit using your Git connection to see preview deployments here.</span>
                    </div>
                ) : (
                    branches.map((deployment) => (
                        <button key={deployment.id} type="button" className="vercel-branch-row" onClick={() => onOpen(deployment.id)}>
                            <Source deployment={deployment} />
                            <span className="vercel-branch-message">{deployment.message}</span>
                            <Status deployment={deployment} duration={formatDuration} />
                            <EnvBadge deployment={deployment} current={false} />
                            <span className="vercel-muted">{formatAgo(deployment.createdAt)}</span>
                            <Avatar name={deployment.author} />
                        </button>
                    ))
                )}
            </section>
            {menu && current && <ContextMenu x={menu.x} y={menu.y} items={actions.menu(current, true)} onClose={() => setMenu(null)} />}
        </div>
    );
}

function latestPerBranch<T extends { branch: string | null; createdAt: number }>(deployments: T[]): T[] {
    const seen = new Set<string>();
    return [...deployments]
        .sort((a, b) => b.createdAt - a.createdAt)
        .filter((deployment) => {
            const branch = deployment.branch ?? "";
            if (!branch || seen.has(branch)) return false;
            seen.add(branch);
            return true;
        });
}
