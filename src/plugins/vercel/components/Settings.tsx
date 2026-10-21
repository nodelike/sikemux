import { formatAgo } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Loaded, Pending } from "../../../plugin-api/ui";
import { vercelDomainsR, vercelEnvR } from "../resources";
import { IconExternal, visit } from "./parts";

const ALL_TARGETS = ["production", "preview", "development"];

const targetsLabel = (targets: string[]) =>
    ALL_TARGETS.every((target) => targets.includes(target))
        ? "All Environments"
        : targets.map((target) => target.charAt(0).toUpperCase() + target.slice(1)).join(", ");

/** A project's environment variables as Vercel lists them, by name only. */
export function Environment({ active, team, project, find }: { active: boolean; team: string; project: string; find: string }) {
    const variables = useResourceEnabled(active, vercelEnvR, team, project);
    if (!variables.data) return <Pending handle={variables} loading="Loading environment variables" rows={5} />;
    const words = find.trim().toLowerCase();
    const shown = variables.data.filter((variable) => !words || variable.key.toLowerCase().includes(words));
    return (
        <div className="vercel-settings">
            <p className="vercel-note">Names only. Values stay in Vercel and are never shown here or given to agents.</p>
            {shown.length === 0 ? (
                <EmptyState message={variables.data.length === 0 ? "No Environment Variables Added" : "No variable matches."} />
            ) : (
                <div className="vercel-table" role="table" aria-label="Environment variables">
                    {shown.map((variable) => (
                        <div key={variable.id} className="vercel-table-row env" role="row">
                            <span className="vercel-cell vercel-mono key">{variable.key}</span>
                            <span className="vercel-cell">
                                {targetsLabel(variable.target)}
                                {variable.gitBranch && <span className="vercel-muted"> · {variable.gitBranch}</span>}
                            </span>
                            <span className="vercel-cell">
                                <span className="vercel-env">{variable.type}</span>
                            </span>
                            <span className="vercel-cell when vercel-muted">
                                {variable.updatedAt ? `Updated ${formatAgo(variable.updatedAt)}` : ""}
                            </span>
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}

/** A project's domains as Vercel lists them, each with whether it is set up right. */
export function Domains({ active, team, project }: { active: boolean; team: string; project: string }) {
    const domains = useResourceEnabled(active, vercelDomainsR, team, project);
    return (
        <Loaded handle={domains} loading="Loading domains" rows={3} empty={(found) => (found.length > 0 ? null : "No domains.")}>
            {(found) => (
                <div className="vercel-settings">
                    {found.map((domain) => (
                        <section key={domain.name} className="vercel-box vercel-domain">
                            <button type="button" className="vercel-link vercel-domain-name" onClick={() => visit(`https://${domain.name}`)}>
                                {domain.name}
                                <IconExternal />
                            </button>
                            <span className="vercel-status">
                                <span className={`vercel-dot ${domain.verified ? "ready" : "error"}`} aria-hidden="true" />
                                {domain.verified ? "Valid Configuration" : "Invalid Configuration"}
                            </span>
                            <span className="vercel-muted">
                                {domain.redirect
                                    ? `Redirects to ${domain.redirect}`
                                    : domain.gitBranch
                                      ? `Assigned to the ${domain.gitBranch} branch`
                                      : "Production"}
                            </span>
                        </section>
                    ))}
                </div>
            )}
        </Loaded>
    );
}
