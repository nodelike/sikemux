import { useState } from "react";
import { copyWithNotice } from "../../../plugin-api/host";
import { formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { ContextMenu, Pending, SendToAgentMenu } from "../../../plugin-api/ui";
import type { VercelLogLine } from "../api";
import { vercelLogV } from "../resources";
import { isUnderway } from "../state";
import { Avatar, EnvBadge, hostOf, IconMore, IconPromote, IconRollback, Source, Status, useDeploymentActions, visit } from "./parts";

/** Builds print plenty to stderr that is not a failure, so only lines that say so are marked. */
const looksLikeError = (line: VercelLogLine) => line.kind === "fatal" || /\b(error|failed)\b|ERR!/i.test(line.text);

/** One deployment as Vercel's details page shows it: its summary, then its build logs. */
export function Deployment({
    paneId,
    active,
    team,
    id,
    production,
    onBack,
}: {
    paneId: string;
    active: boolean;
    team: string;
    id: string;
    production: string | null;
    onBack: () => void;
}) {
    const log = useResourceEnabled(active, vercelLogV, paneId, team, id);
    const actions = useDeploymentActions(team);
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const [agentMenu, setAgentMenu] = useState<{ x: number; y: number } | null>(null);
    const read = log.data?.log;
    const deployment = read?.deployment;
    const underway = deployment ? isUnderway(deployment.state) : false;

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
                {deployment.error && <p className="vercel-error">{deployment.error}</p>}
            </section>

            <section className="vercel-box vercel-logs">
                <header className="vercel-box-head">
                    <span className="vercel-box-title">Build Logs</span>
                    <span className="vercel-muted">
                        {read && read.skipped > 0
                            ? `last ${read.lines.length} of ${read.lines.length + read.skipped} lines`
                            : `${read?.lines.length ?? 0} lines`}
                    </span>
                </header>
                <div className="vercel-log" role="log" aria-label="Build log">
                    {read && read.lines.length === 0 ? (
                        <p className="vercel-note">{underway ? "Waiting for the build to print…" : "This build printed nothing."}</p>
                    ) : (
                        read?.lines.map((line, index) => (
                            <div key={index} className={`vercel-log-line ${line.kind}${looksLikeError(line) ? " error" : ""}`}>
                                <span className="vercel-log-time">{new Date(line.at).toLocaleTimeString(undefined, { hour12: false })}</span>
                                <span>{line.text || " "}</span>
                            </div>
                        ))
                    )}
                </div>
            </section>
            {menu && <ContextMenu x={menu.x} y={menu.y} items={actions.menu(deployment, current)} onClose={() => setMenu(null)} />}
            {agentMenu && (
                <SendToAgentMenu
                    x={agentMenu.x}
                    y={agentMenu.y}
                    alignRight
                    delivery={() => ({ text: log.data?.forAgent ?? "" })}
                    onClose={() => setAgentMenu(null)}
                />
            )}
        </div>
    );
}
