import { EmptyState } from "../../../plugin-api/ui";
import type { VercelProject } from "../api";
import { Ago, hostOf, IconBranch, IconGitHub, StateDot, stateLabel } from "./parts";

/** Every project of the team as cards, as on Vercel's dashboard. */
export function Projects({ projects, find, onOpen }: { projects: VercelProject[]; find: string; onOpen: (name: string) => void }) {
    const words = find.trim().toLowerCase();
    const shown = projects.filter((project) => !words || project.name.toLowerCase().includes(words) || project.repo?.toLowerCase().includes(words));
    if (shown.length === 0) return <EmptyState message={words ? "No project matches." : "No projects here yet."} />;
    return (
        <div className="vercel-cards">
            {shown.map((project) => (
                <button key={project.id} type="button" className="vercel-card project" onClick={() => onOpen(project.name)}>
                    <span className="vercel-card-head">
                        <span className="vercel-card-icon" aria-hidden="true">
                            {project.name.charAt(0).toUpperCase()}
                        </span>
                        <span className="vercel-card-name">
                            <span className="vercel-card-title">{project.name}</span>
                            <span className="vercel-muted">{hostOf(project.production?.url ?? null) ?? "No production deployment"}</span>
                        </span>
                        {project.production && (
                            <span className="vercel-status small" title={stateLabel(project.production.state)}>
                                <StateDot state={project.production.state} />
                            </span>
                        )}
                    </span>
                    {project.repo && (
                        <span className="vercel-repo-pill">
                            <IconGitHub />
                            {project.repo}
                        </span>
                    )}
                    <span className="vercel-card-foot">
                        <Ago at={project.updatedAt} />
                        {project.productionBranch && (
                            <span className="vercel-source-part">
                                <IconBranch />
                                <span className="vercel-mono">{project.productionBranch}</span>
                            </span>
                        )}
                    </span>
                </button>
            ))}
        </div>
    );
}
