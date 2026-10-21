import type { ReactNode } from "react";
import { prettyPath } from "../lib/paths";
import { LANGUAGE_NAMES, type ProjectFolder } from "../projects/projectFolders";
import { driftsOf } from "../projects/projectHealth";
import { changeProject, javaRuntimesR, projectFoldersR, projectSuggestionsR, useProjectSettingsSync } from "../projects/projectResources";
import { isPending } from "../projects/projectSuggestions";
import * as cmd from "../state/commands";
import { useResourceEnabled } from "../state/resources";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import {
    IconAgent,
    IconCheck,
    IconCode,
    IconFolder,
    IconGit,
    IconLock,
    IconRun,
    IconServer,
    IconSettings,
    IconSparkle,
    IconTerminal,
} from "../ui/Icons";
import { PaneField } from "../ui/ShaderField";
import { ProjectBriefPage } from "./ProjectBriefPage";
import { ProjectDonePage } from "./ProjectDonePage";
import { ProjectEnvironmentPage } from "./ProjectEnvironmentPage";
import { ProjectGeneralPage, ProjectGitPage } from "./ProjectGeneralPage";
import { ProjectGuardrailsPage } from "./ProjectGuardrailsPage";
import { ProjectOverviewPage } from "./ProjectOverviewPage";
import { ProjectServicesPage } from "./ProjectServicesPage";
import { ProjectStackPage } from "./ProjectStackPage";
import { ProjectSuggestionsPage } from "./ProjectSuggestionsPage";
import { ProjectTasksPage } from "./ProjectTasksPage";
import { stackOfPage, stackPage, type ProjectSettingsPageId } from "./projectSettingsPages";
import "../styles/settings.css";

const PAGE_TITLES: Partial<Record<ProjectSettingsPageId, string>> = {
    overview: "Overview",
    services: "Services",
    guardrails: "Guardrails",
    environment: "Environment",
    tasks: "Tasks",
    done: "Done means",
    brief: "What agents know",
    suggestions: "Suggestions",
    general: "General",
    git: "Git",
};

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

/** A project's own settings, in a tab of its own: how its code is built and run, and how Sikemux treats it. */
export function ProjectSettingsPane({ project, name, visible }: { project: string; name: string; visible: boolean }) {
    const asked = useStore((s) => s.projectSettingsPages[project]) ?? "start";
    const home = useStore((s) => s.home);
    const paneShader = useStore((s) => s.paneShader);
    useProjectSettingsSync();
    const folders = useResourceEnabled(visible, projectFoldersR, project).data;
    const suggestions = useResourceEnabled(visible, projectSuggestionsR, project).data ?? [];
    const waiting = suggestions.filter(isPending).length;
    const stacks = folders ? stacksOf(folders.folders) : [];
    const page: ProjectSettingsPageId = asked === "start" ? "overview" : asked;
    const runtimes =
        useResourceEnabled(
            Object.values(folders?.settings ?? {}).some((chosen) => chosen.env?.JAVA_HOME),
            javaRuntimesR,
        ).data ?? null;
    const drifts = folders ? driftsOf(folders.folders, folders.settings, runtimes) : [];
    const stackAt = stackOfPage(page);
    const stack = stackAt && stacks.find((entry) => entry.folder.path === stackAt.folder && entry.language.id === stackAt.language);
    const title = stack ? stack.label : (PAGE_TITLES[page] ?? "Environment");

    const setVariable = (folder: string, key: string, value: string | null) =>
        void changeProject(project, { kind: "variable", key, value }, folder).catch(reportError("Save the project's settings"));

    const item = (id: ProjectSettingsPageId, label: string, icon: ReactNode, badge?: string) => (
        <button
            key={id}
            type="button"
            className={`settings-rail-item${page === id ? " active" : ""}`}
            aria-current={page === id ? "page" : undefined}
            onClick={() => cmd.setProjectSettingsPage(project, id)}>
            <span className="settings-rail-icon" aria-hidden="true">
                {icon}
            </span>
            <span className="settings-rail-name">{label}</span>
            {badge && <span className="project-settings-badge">{badge}</span>}
        </button>
    );

    return (
        <div className="settings-pane in-tab" aria-label={`${name} settings`}>
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
                            {item("services", "Services", <IconServer size={13} />)}
                            {item("tasks", "Tasks", <IconRun size={13} />)}
                            {item("done", "Done means", <IconCheck size={13} />)}
                        </div>
                        <div className="settings-nav-group">
                            <span className="settings-nav-label">Agents</span>
                            {item("brief", "What agents know", <IconAgent size={13} />)}
                            {item("guardrails", "Guardrails", <IconLock size={13} />)}
                            {item("suggestions", "Suggestions", <IconSparkle size={13} />, waiting ? String(waiting) : undefined)}
                        </div>
                        <div className="settings-nav-group">
                            <span className="settings-nav-label">Sikemux</span>
                            {item("general", "General", <IconSettings size={13} />)}
                            {item("git", "Git", <IconGit size={13} />)}
                        </div>
                    </nav>
                    <div className="settings-rail-foot">Changes save automatically</div>
                </aside>

                <div className="settings-main">
                    <PaneField enabled={paneShader} />
                    <header className="settings-topbar">
                        <span className="settings-topbar-title">{title}</span>
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
                                fields={folders.fields[stack.language.id] ?? []}
                                settings={folders.settings[stack.folder.path]}
                                setVariable={(key, value) => setVariable(stack.folder.path, key, value)}
                            />
                        )}
                        {page === "environment" && folders && (
                            <ProjectEnvironmentPage project={project} name={name} folders={folders.folders} settings={folders.settings} />
                        )}
                        {page === "tasks" && <ProjectTasksPage project={project} />}
                        {page === "services" && folders && (
                            <ProjectServicesPage project={project} name={name} folders={folders.folders} settings={folders.settings} />
                        )}
                        {page === "done" && folders && (
                            <ProjectDonePage project={project} name={name} settings={folders.settings} rules={folders.rules} />
                        )}
                        {page === "guardrails" && folders && <ProjectGuardrailsPage project={project} rules={folders.rules} />}
                        {page === "suggestions" && <ProjectSuggestionsPage project={project} name={name} suggestions={suggestions} />}
                        {page === "brief" && folders && <ProjectBriefPage project={project} rules={folders.rules} />}
                        {page === "general" && <ProjectGeneralPage project={project} />}
                        {page === "git" && <ProjectGitPage project={project} visible={visible} />}
                    </div>
                </div>
            </div>
        </div>
    );
}
