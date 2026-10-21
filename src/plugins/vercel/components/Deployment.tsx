import { useState } from "react";
import { copyWithNotice } from "../../../plugin-api/host";
import { formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, Pending, SendToAgentMenu } from "../../../plugin-api/ui";
import type { VercelLogLine } from "../api";
import { vercelLogR } from "../resources";
import { useRefreshEvery } from "../../../plugin-api/hooks";
import { deploymentForAgent, isUnderway, UNDERWAY_REFRESH_MS } from "../state";
import { Avatar, EnvBadge, hostOf, IconMore, IconPromote, IconRollback, Source, Status, useDeploymentActions, visit } from "./parts";
import { Protection } from "./Protection";
import { RuntimeLogs } from "./RuntimeLogs";

/** Builds print plenty to stderr that is not a failure, so only lines that say so are marked. */
const looksLikeError = (line: VercelLogLine) => line.kind === "fatal" || /\b(error|failed)\b|ERR!/i.test(line.text);

/** One deployment as Vercel's details page shows it: its summary, then its build logs. */
export function Deployment({
    active,
    team,
    id,
    production,
    onBack,
}: {
    active: boolean;
    team: string;
    id: string;
    production: string | null;
    onBack: () => void;
}) {
    const log = useResourceEnabled(active, vercelLogR, team, id);
    const actions = useDeploymentActions(team);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [agentMenu, setAgentMenu] = useState<{ x: number; y: number } | null>(null);
    const deployment = log.data?.deployment;
    const underway = deployment ? isUnderway(deployment.state) : false;
    useRefreshEvery(log, active && underway ? UNDERWAY_REFRESH_MS : null);

    if (!deployment) return <Pending handle={log} loading={`Loading ${id}`} rows={8} />;

    const current = deployment.id === production;
    const ready = deployment.state === "READY";
    const aliases = deployment.aliases ?? [];

    return (
        <div className="vercel-detail-page" aria-label={`Deployment ${hostOf(deployment.url) ?? deployment.id}`}>
            <div className="vercel-detail-bar">
                <button type="button" className="vercel-link" onClick={onBack}>
                    ← Deployments
                </button>
                <span className="vercel-actions">
                    {deployment.target === "production" && ready && !current && (
                        <button type="button" className="vercel-button" disabled={actions.busy} onClick={() => void actions.rollback(deployment)}>
                            <IconRollback />
                            Instant Rollback
                        </button>
                    )}
                    {deployment.target === "preview" && ready && (
                        <button type="button" className="vercel-button" disabled={actions.busy} onClick={() => void actions.promote(deployment)}>
                            <IconPromote />
                            Promote
                        </button>
                    )}
                    {underway ? (
                        <button type="button" className="vercel-button" disabled={actions.busy} onClick={() => void actions.cancel(deployment)}>
                            Cancel
                        </button>
                    ) : (
                        <button type="button" className="vercel-button" disabled={actions.busy} onClick={() => void actions.redeploy(deployment)}>
                            Redeploy
                        </button>
                    )}
                    <button type="button" className="vercel-button" onClick={(event) => setAgentMenu({ x: event.clientX, y: event.clientY })}>
                        Send to agent
                    </button>
                    {deployment.url && ready && (
                        <button type="button" className="vercel-button primary" onClick={() => visit(deployment.url!)}>
                            Visit
                        </button>
                    )}
                    <button
                        type="button"
                        className="vercel-button icon"
                        aria-label="More actions"
                        onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                        <IconMore />
                    </button>
                </span>
            </div>

            <section className="vercel-box">
                <h2 className="vercel-detail-title">{deployment.message || hostOf(deployment.url) || deployment.id}</h2>
                <dl className="vercel-facts grid">
                    <div>
                        <dt>Created</dt>
                        <dd>
                            {formatAgo(deployment.createdAt)}
                            {deployment.author && (
                                <>
                                    {" "}
                                    by {deployment.author} <Avatar name={deployment.author} />
                                </>
                            )}
                        </dd>
                    </div>
                    <div>
                        <dt>Status</dt>
                        <dd>
                            <Status deployment={deployment} duration={formatDuration} />
                        </dd>
                    </div>
                    <div>
                        <dt>Environment</dt>
                        <dd>
                            <EnvBadge deployment={deployment} current={current} />
                            {current && <span className="vercel-muted">Current</span>}
                        </dd>
                    </div>
                    <div>
                        <dt>Duration</dt>
                        <dd>{deployment.buildMs !== null ? formatDuration(deployment.buildMs) : underway ? "Building…" : "—"}</dd>
                    </div>
                    <div className="wide">
                        <dt>Domains</dt>
                        <dd className="vercel-domain-list">
                            {deployment.url && (
                                <button type="button" className="vercel-link" onClick={() => visit(deployment.url!)}>
                                    {hostOf(deployment.url)}
                                </button>
                            )}
                            {aliases.map((alias) => (
                                <button key={alias} type="button" className="vercel-link" onClick={() => visit(`https://${alias}`)}>
                                    {alias}
                                </button>
                            ))}
                        </dd>
                    </div>
                    <div className="wide">
                        <dt>Source</dt>
                        <dd className="vercel-source-block">
                            <Source deployment={deployment} />
                            {deployment.commit && (
                                <button type="button" className="vercel-link" onClick={() => copyWithNotice(deployment.commit!, "the commit")}>
                                    {deployment.message || deployment.commit.slice(0, 7)}
                                </button>
                            )}
                        </dd>
                    </div>
                </dl>
                <Protection active={active} team={team} deployment={deployment} />
                {deployment.error && <p className="vercel-error">{deployment.error}</p>}
            </section>

            <section className="vercel-box vercel-logs">
                <header className="vercel-box-head">
                    <span className="vercel-box-title">Build Logs</span>
                    <span className="vercel-muted">
                        {log.data && log.data.skipped > 0
                            ? `last ${log.data.lines.length} of ${log.data.lines.length + log.data.skipped} lines`
                            : `${log.data?.lines.length ?? 0} lines`}
                    </span>
                </header>
                <div className="vercel-log" role="log" aria-label="Build log">
                    {log.data && log.data.lines.length === 0 ? (
                        <p className="vercel-note">{underway ? "Waiting for the build to print…" : "This build printed nothing."}</p>
                    ) : (
                        log.data?.lines.map((line, index) => (
                            <div key={index} className={`vercel-log-line ${line.kind}${looksLikeError(line) ? " error" : ""}`}>
                                <span className="vercel-log-time">{new Date(line.at).toLocaleTimeString(undefined, { hour12: false })}</span>
                                <span>{line.text || " "}</span>
                            </div>
                        ))
                    )}
                </div>
            </section>
            {ready ? <RuntimeLogs team={team} deployment={deployment} /> : null}
            {menu && <ContextMenu x={menu.x} y={menu.y} items={actions.menu(deployment, current)} onClose={() => setMenu(null)} />}
            {agentMenu && (
                <SendToAgentMenu
                    x={agentMenu.x}
                    y={agentMenu.y}
                    alignRight
                    delivery={() => ({ text: deploymentForAgent(deployment, log.data) })}
                    onClose={() => setAgentMenu(null)}
                />
            )}
        </div>
    );
}
