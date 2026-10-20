import { useEffect, type ReactNode } from "react";
import { prettyPath } from "../lib/paths";
import { LANGUAGE_NAMES, saveFolderSettings, useProjectFolders, type ProjectFolder } from "../projects/projectFolders";
import { driftsOf, useJavaRuntimes } from "../projects/projectHealth";
import { isPending, useProjectSuggestions } from "../projects/projectSuggestions";
import * as cmd from "../state/commands";
import { useStore, type StoreState } from "../state/store";
import { reportError } from "../state/toast";
import { IconAgent, IconCheck, IconClose, IconCode, IconFolder, IconRun, IconSparkle, IconTerminal } from "../ui/Icons";
import { PaneField } from "../ui/ShaderField";
import { useLeaveOnNavigation } from "./leaveSettings";
import { ProjectBriefPage, ProjectDonePage, ProjectSuggestionsPage } from "./ProjectAgentPages";
import { ProjectEnvironmentPage } from "./ProjectEnvironmentPage";
import { ProjectOverviewPage } from "./ProjectOverviewPage";
import { ProjectStackPage } from "./ProjectStackPage";
import { ProjectTasksPage } from "./ProjectTasksPage";
import { stackOfPage, stackPage, type ProjectSettingsPageId } from "./projectSettingsPages";
import "../styles/settings.css";

const isOpen = (state: StoreState) => state.projectSettingsPage !== null;

const PAGE_TITLES: Partial<Record<ProjectSettingsPageId, string>> = {
    overview: "Overview",
    environment: "Environment",
    tasks: "Tasks",
    done: "Done means",
    brief: "What agents know",
    suggestions: "Suggestions",
};

/** The project in front's own settings: how its code is built and run, standing in for its workspace. */
export function ProjectSettingsPanel() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);

    useLeaveOnNavigation(isOpen, cmd.closeProjectSettings);

    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            cmd.closeProjectSettings();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    if (session?.kind !== "project") return null;
    return <ProjectSettings project={session.cwd} name={session.name} />;
}

interface Stack {
    page: ProjectSettingsPageId;
    folder: ProjectFolder;
    language: ProjectFolder["languages"][number];
    label: string;
}

/** One page for each language in each folder; a project with one stack at its root just names the language. */
function stacksOf(folders: readonly ProjectFolder[]): Stack[] {
    const all = folders.flatMap((folder) => folder.languages.map((language) => ({ folder, language })));
    const alone = all.length === 1 && all[0].folder.path === ".";
    return all.map(({ folder, language }) => ({
        page: stackPage(folder.path, language.id),
        folder,
        language,
        label: alone ? LANGUAGE_NAMES[language.id] : `${LANGUAGE_NAMES[language.id]} · ${folder.path === "." ? "root" : folder.path}`,
    }));
}

function ProjectSettings({ project, name }: { project: string; name: string }) {
    const asked = useStore((s) => s.projectSettingsPage) ?? "start";
    const home = useStore((s) => s.home);
    const paneShader = useStore((s) => s.paneShader);
    const { folders, refresh: refreshFolders } = useProjectFolders(project);
    const { suggestions, refresh: refreshSuggestions } = useProjectSuggestions(project);
    const waiting = suggestions.filter(isPending).length;
    const refresh = () => {
        refreshFolders();
        refreshSuggestions();
    };
    const stacks = folders ? stacksOf(folders.folders) : [];
    const page: ProjectSettingsPageId = asked === "start" ? "overview" : asked;
    const runtimes = useJavaRuntimes(Object.values(folders?.settings ?? {}).some((chosen) => chosen.env?.JAVA_HOME));
    const drifts = folders ? driftsOf(folders.folders, folders.settings, runtimes) : [];
    const stackAt = stackOfPage(page);
    const stack = stackAt && stacks.find((entry) => entry.folder.path === stackAt.folder && entry.language.id === stackAt.language);
    const title = stack ? stack.label : (PAGE_TITLES[page] ?? "Environment");

    const setVariable = (folder: string, key: string, value: string | null) => {
        const current = folders?.settings[folder] ?? {};
        const { [key]: _replaced, ...rest } = current.env ?? {};
        const env = value === null ? rest : { ...rest, [key]: value };
        void saveFolderSettings(project, folder, { ...current, env })
            .then(refresh)
            .catch(reportError("Save the project's settings"));
    };

    const item = (id: ProjectSettingsPageId, label: string, icon: ReactNode, badge?: string) => (
        <button
            key={id}
            type="button"
            className={`settings-rail-item${page === id ? " active" : ""}`}
            aria-current={page === id ? "page" : undefined}
            onClick={() => cmd.openProjectSettings(id)}>
            <span className="settings-rail-icon" aria-hidden="true">
                {icon}
            </span>
            <span className="settings-rail-name">{label}</span>
            {badge && <span className="project-settings-badge">{badge}</span>}
        </button>
    );

    return (
        <div tabIndex={-1} className="settings-pane" role="dialog" aria-label={`${name} settings`}>
            <div className="settings-frame">
                <aside className="settings-rail">
                    <div className="project-settings-head">
                        <IconFolder size={13} />
                        <span className="project-settings-name">{name}</span>
                        <span className="project-settings-path">{prettyPath(project, home)}</span>
                    </div>
                    <nav className="settings-nav" aria-label="Project settings sections">
                        <div className="settings-nav-group">
                            {item(
                                "overview",
                                "Overview",
                                <IconFolder size={13} />,
                                waiting + drifts.length ? String(waiting + drifts.length) : undefined,
                            )}
                        </div>
                        {stacks.length > 0 && (
                            <div className="settings-nav-group">
                                <span className="settings-nav-label">Code</span>
                                {stacks.map((entry) => item(entry.page, entry.label, <IconCode size={13} />))}
                            </div>
                        )}
                        <div className="settings-nav-group">
                            <span className="settings-nav-label">Run</span>
                            {item("environment", "Environment", <IconTerminal size={13} />)}
                            {item("tasks", "Tasks", <IconRun size={13} />)}
                            {item("done", "Done means", <IconCheck size={13} />)}
                        </div>
                        <div className="settings-nav-group">
                            <span className="settings-nav-label">Agents</span>
                            {item("brief", "What agents know", <IconAgent size={13} />)}
                            {item("suggestions", "Suggestions", <IconSparkle size={13} />, waiting ? String(waiting) : undefined)}
                        </div>
                    </nav>
                    <div className="settings-rail-foot">Changes save automatically</div>
                </aside>

                <div className="settings-main">
                    <PaneField enabled={paneShader} />
                    <header className="settings-topbar">
                        <span className="settings-topbar-title">{title}</span>
                        <button
                            className="settings-topbar-close"
                            onClick={cmd.closeProjectSettings}
                            title="Close project settings (Esc)"
                            aria-label="Close project settings"
                            type="button">
                            <IconClose size={14} />
                        </button>
                    </header>
                    <div className="settings-scroll">
                        {page === "overview" && folders && (
                            <ProjectOverviewPage
                                project={project}
                                name={name}
                                settings={folders.settings}
                                waiting={suggestions.filter(isPending)}
                                drifts={drifts}
                                fixDrift={(drift) => setVariable(drift.folder, drift.fix.key, drift.fix.value)}
                            />
                        )}
                        {stack && folders && (
                            <ProjectStackPage
                                drifts={drifts.filter((drift) => drift.folder === stack.folder.path && drift.language === stack.language.id)}
                                onFixDrift={(drift) => setVariable(drift.folder, drift.fix.key, drift.fix.value)}
                                key={stack.page}
                                where={stack.folder.path === "." ? "the project" : `${stack.folder.path}/`}
                                language={stack.language}
                                settings={folders.settings[stack.folder.path]}
                                setVariable={(key, value) => setVariable(stack.folder.path, key, value)}
                            />
                        )}
                        {page === "environment" && folders && (
                            <ProjectEnvironmentPage
                                project={project}
                                name={name}
                                folders={folders.folders}
                                settings={folders.settings}
                                onChanged={refresh}
                            />
                        )}
                        {page === "tasks" && <ProjectTasksPage project={project} />}
                        {page === "done" && folders && (
                            <ProjectDonePage project={project} name={name} settings={folders.settings} rules={folders.rules} onChanged={refresh} />
                        )}
                        {page === "suggestions" && (
                            <ProjectSuggestionsPage project={project} name={name} suggestions={suggestions} onChanged={refresh} />
                        )}
                        {page === "brief" && folders && (
                            <ProjectBriefPage
                                project={project}
                                folders={folders.folders}
                                settings={folders.settings}
                                rules={folders.rules}
                                onChanged={refresh}
                            />
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
}
