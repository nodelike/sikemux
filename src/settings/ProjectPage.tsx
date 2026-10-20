import { useMemo } from "react";
import { enabledCodeHosts } from "../codehost/registry";
import { forgetProjectCodeHost } from "../codehost/state";
import { allowLanguageServers } from "../editor/languageServerTrust";
import { useProjectRepo } from "../git/projectRepo";
import { prettyPath } from "../lib/paths";
import { SpaceGlyph } from "../rail/SpaceSwitch";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { IconFolder, IconGit } from "../ui/Icons";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

interface OpenProject {
    cwd: string;
    name: string;
}

function useOpenProjects(): OpenProject[] {
    const sessions = useStore((s) => s.sessions);
    const order = useStore((s) => s.sessionOrder);
    return useMemo(
        () =>
            order.flatMap((id) => {
                const session = sessions[id];
                return session?.kind === "project" && session.cwd ? [{ cwd: session.cwd, name: session.name }] : [];
            }),
        [sessions, order],
    );
}

function useFrontProject(): string {
    return useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" ? session.cwd : "";
    });
}

/** The project the page edits: the one picked while it is still open, else the one in front. */
function useSettingsProject(projects: readonly OpenProject[]): string {
    const picked = useStore((s) => s.settingsProject);
    const front = useFrontProject();
    if (picked && projects.some((project) => project.cwd === picked)) return picked;
    return front || projects[0]?.cwd || "";
}

/** Picking the project in front goes back to following whichever project is in front. */
export function ProjectSettingsPicker() {
    const projects = useOpenProjects();
    const project = useSettingsProject(projects);
    const front = useFrontProject();
    const home = useStore((s) => s.home);
    if (!projects.length) return <p className="settings-project-none">No project open</p>;
    return (
        <Dropdown
            className="settings-dd settings-project-picker"
            label="Project to change"
            icon={<IconFolder size={12} />}
            value={project}
            search={projects.length > 6 ? "Find a project" : undefined}
            options={projects.map((open) => ({
                value: open.cwd,
                label: open.name,
                detail: prettyPath(open.cwd, home),
                meta: open.cwd === front ? "in front" : undefined,
            }))}
            onChange={(cwd) => cmd.setSettingsProject(cwd === front ? null : cwd)}
        />
    );
}

/** Settings that belong to one open project, kept on this host rather than in the repository. */
export function ProjectPage() {
    const projects = useOpenProjects();
    const project = useSettingsProject(projects);
    return (
        <SettingsPage>
            <DefaultRepoSection project={project} />
            <LanguageServersSection project={project} />
            <CodeHostsSection project={project} />
            <SpaceSection project={project} />
        </SettingsPage>
    );
}

function DefaultRepoSection({ project }: { project: string }) {
    const { repo, repos } = useProjectRepo(project, !!project);
    const chosen = useStore((s) => s.defaultGitRepos[project]);
    return (
        <SettingsSection
            title="Default git repository"
            meta={repos?.length ? `${repos.length} inside` : undefined}
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
    );
}

const LANGUAGE_SERVER_STATE = {
    unasked: "Sikemux asks the first time you open a file here that one could help with.",
    allowed: "They start when you open a file here. They can run the project's build scripts and installed tools.",
    refused: "Turned off for this project. The editor still highlights code, without completions or go to definition.",
};

function LanguageServersSection({ project }: { project: string }) {
    const trust = useStore((s) => s.languageServerTrust[project]);
    const state = trust === undefined ? "unasked" : trust ? "allowed" : "refused";
    return (
        <SettingsSection title="Language servers">
            <SettingsRows>
                <SettingsRow
                    asLabel
                    label="Let this project start language servers"
                    desc={project ? LANGUAGE_SERVER_STATE[state] : "Open a project to change this."}
                    control={<Switch checked={trust === true} disabled={!project} onChange={(on) => allowLanguageServers(project, on)} />}
                />
            </SettingsRows>
        </SettingsSection>
    );
}

function CodeHostsSection({ project }: { project: string }) {
    const picks = useStore((s) => s.codeHosts[project]);
    const hosts = enabledCodeHosts();
    return (
        <SettingsSection title="Code hosts" sub="Without a pick, each host follows the project's remote and the account you signed in with.">
            {!project ? (
                <p className="settings-repo-note">Open a project to see what it picked on each host.</p>
            ) : !hosts.length ? (
                <p className="settings-repo-note">Turn on a code host, such as GitHub, in Plugins.</p>
            ) : (
                <SettingsRows>
                    {hosts.map((host) => {
                        const pick = picks?.[host.id];
                        const picked = [pick?.repo, pick?.account && `as ${pick.account}`].filter(Boolean).join(" ");
                        return (
                            <SettingsRow
                                key={host.id}
                                icon={host.icon(14)}
                                label={<span>{host.name}</span>}
                                desc={
                                    picked ? (
                                        <span className="settings-code-host-pick">{picked}</span>
                                    ) : (
                                        "Found from the remote. Nothing picked by hand."
                                    )
                                }
                                control={
                                    picked ? (
                                        <button type="button" className="settings-btn" onClick={() => forgetProjectCodeHost(host.id, project)}>
                                            Forget
                                        </button>
                                    ) : undefined
                                }
                            />
                        );
                    })}
                </SettingsRows>
            )}
        </SettingsSection>
    );
}

function SpaceSection({ project }: { project: string }) {
    const spaces = useStore((s) => s.spaces);
    const space = useStore((s) => s.projectSpaces[project] ?? "");
    return (
        <SettingsSection title="Space">
            <SettingsRows>
                <SettingsRow
                    label="Listed under"
                    desc={
                        spaces.length
                            ? "The rail shows this project when its space is chosen, and always under All."
                            : "Make a space from the switcher at the top of the rail."
                    }
                    wide>
                    <Dropdown
                        className="settings-dd"
                        label="Space"
                        value={space}
                        disabled={!project || !spaces.length}
                        options={[
                            { value: "", label: "No space" },
                            ...spaces.map((entry) => ({ value: entry.id, label: entry.name, icon: <SpaceGlyph icon={entry.icon} /> })),
                        ]}
                        onChange={(id) => cmd.setProjectSpace(project, id || null)}
                    />
                </SettingsRow>
            </SettingsRows>
        </SettingsSection>
    );
}
