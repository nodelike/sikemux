import { allowLanguageServers } from "../editor/languageServerTrust";
import { useProjectRepo } from "../git/projectRepo";
import { basename } from "../lib/paths";
import { SpaceGlyph } from "../rail/SpaceSwitch";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { IconGit } from "../ui/Icons";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

/** How Sikemux itself treats the project: worktrees for new chats, language servers and its Space. */
export function ProjectGeneralPage({ project }: { project: string }) {
    const worktree = useStore((s) => s.agentWorktreeDefaults[project] === true);
    const languageServers = useStore((s) => s.languageServerTrust[project] === true);
    const spaces = useStore((s) => s.spaces);
    const space = useStore((s) => s.projectSpaces[project] ?? "");
    return (
        <SettingsPage>
            <SettingsSection title="General" sub="Kept in Sikemux, not in the repository.">
                <SettingsRows>
                    <SettingsRow
                        label="New chats start in a worktree"
                        desc="An agent gets its own checkout, so its edits stay off your branch until you merge them."
                        asLabel>
                        <Switch checked={worktree} onChange={(on) => cmd.setAgentWorktreeDefault(project, on)} />
                    </SettingsRow>
                    <SettingsRow
                        label="Language servers"
                        desc="They can run code from the project, such as its build scripts. Only allow them for projects you trust."
                        asLabel>
                        <Switch checked={languageServers} onChange={(on) => allowLanguageServers(project, on)} />
                    </SettingsRow>
                    <SettingsRow label="Space" desc="The rail shows the project under this space." wide>
                        <Dropdown
                            className="settings-dd"
                            label="Space"
                            value={space}
                            options={[
                                { value: "", label: "No Space" },
                                ...spaces.map((entry) => ({ value: entry.id, label: entry.name, icon: <SpaceGlyph icon={entry.icon} /> })),
                            ]}
                            onChange={(value) => cmd.setProjectSpace(project, value || null)}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}

/** Which repository the project's Git views use, when its folder holds several. */
export function ProjectGitPage({ project, visible }: { project: string; visible: boolean }) {
    const { repo, repos } = useProjectRepo(project, visible);
    const chosen = useStore((s) => s.defaultGitRepos[project]);
    return (
        <SettingsPage>
            <SettingsSection
                title="Default git repository"
                sub="When the project folder holds several repositories, its Git pane, the branch in the top bar, the diff pane and new agent worktrees use this one.">
                {repos === null ? (
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
                        {repo && <p className="settings-repo-note">Git views open {basename(repo)}.</p>}
                    </div>
                )}
            </SettingsSection>
        </SettingsPage>
    );
}
