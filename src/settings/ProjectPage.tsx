import { useProjectRepo } from "../git/projectRepo";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { IconGit } from "../ui/Icons";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

/** Settings that belong to the project open in front, kept on this Mac rather than in the repository. */
export function ProjectPage() {
    const project = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" ? session.cwd : "";
    });
    const { repo, repos } = useProjectRepo(project, !!project);
    const chosen = useStore((s) => (project ? s.defaultGitRepos[project] : undefined));
    const name = project.split("/").pop() ?? project;

    return (
        <SettingsPage>
            <SettingsSection
                title="Default git repository"
                meta={project ? name : undefined}
                sub="When the project folder holds several repositories, its Git pane, the branch in the top bar, the diff pane and new agent worktrees use this one.">
                {!project ? (
                    <p className="settings-repo-note">Open a project to choose its repository.</p>
                ) : repos === null ? (
                    <p className="settings-repo-note">This project folder is a repository itself, so its Git views use it.</p>
                ) : repos.length === 0 ? (
                    <p className="settings-repo-note">There is no repository in this folder or the folders directly inside it.</p>
                ) : (
                    <div className="settings-repo-choices" role="radiogroup" aria-label="Default git repository">
                        <button
                            type="button"
                            role="radio"
                            aria-checked={!chosen}
                            className={`settings-repo-choice${!chosen ? " on" : ""}`}
                            onClick={() => cmd.setDefaultGitRepo(project, null)}>
                            <span className="settings-repo-name">{repos.length === 1 ? "The only one inside" : "Ask each time"}</span>
                            <span className="settings-repo-branch">{repos.length === 1 ? repos[0].name : "the Git pane lists them"}</span>
                        </button>
                        {repos.map((entry) => (
                            <button
                                key={entry.path}
                                type="button"
                                role="radio"
                                aria-checked={chosen === entry.path}
                                className={`settings-repo-choice${chosen === entry.path ? " on" : ""}`}
                                onClick={() => cmd.setDefaultGitRepo(project, entry.path)}>
                                <IconGit size={12} />
                                <span className="settings-repo-name">{entry.name}</span>
                                <span className="settings-repo-branch">{entry.branch || "no commits"}</span>
                                {entry.changes > 0 && <span className="settings-repo-changes">{entry.changes} changed</span>}
                            </button>
                        ))}
                        {repo && <p className="settings-repo-note">Git views open {repo.split("/").pop()}.</p>}
                    </div>
                )}
            </SettingsSection>
        </SettingsPage>
    );
}
