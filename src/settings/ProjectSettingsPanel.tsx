import { useEffect, type ReactNode } from "react";
import { prettyPath } from "../lib/paths";
import { folderName, LANGUAGE_NAMES, useProjectFolders } from "../projects/projectFolders";
import * as cmd from "../state/commands";
import { useStore, type StoreState } from "../state/store";
import { IconAgent, IconClose, IconFile, IconFolder } from "../ui/Icons";
import { PaneField } from "../ui/ShaderField";
import { useLeaveOnNavigation } from "./leaveSettings";
import { ProjectFilePage } from "./ProjectFilePage";
import { ProjectFolderPage } from "./ProjectFolderPage";
import { ProjectAgentsPage, ProjectGeneralPage } from "./ProjectPage";
import { folderOfPage, folderPage, PROJECT_SETTINGS_PAGES, type ProjectSettingsPageId } from "./projectSettingsPages";
import "../styles/settings.css";

const PAGE_ICONS: Record<string, ReactNode> = {
    general: <IconFolder size={13} />,
    agents: <IconAgent size={13} />,
    file: <IconFile size={13} />,
};

const isOpen = (state: StoreState) => state.projectSettingsPage !== null;

/** The project in front's own settings, standing in for its workspace as the app's settings do. */
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

function ProjectSettings({ project, name }: { project: string; name: string }) {
    const page = useStore((s) => s.projectSettingsPage) ?? "general";
    const home = useStore((s) => s.home);
    const paneShader = useStore((s) => s.paneShader);
    const { folders, refresh } = useProjectFolders(project);
    const folderPath = folderOfPage(page);
    const folder = folderPath === null ? null : folders?.folders.find((entry) => entry.path === folderPath);
    const title = folder ? folderName(folder.path, name) : PROJECT_SETTINGS_PAGES.find((entry) => entry.id === page)?.name;

    const item = (id: ProjectSettingsPageId, label: string, icon: ReactNode, meta?: string) => (
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
            {meta && <span className="project-settings-meta">{meta}</span>}
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
                            {PROJECT_SETTINGS_PAGES.map((entry) => item(entry.id, entry.name, PAGE_ICONS[entry.id]))}
                        </div>
                        {folders && folders.folders.length > 0 && (
                            <div className="settings-nav-group">
                                <span className="settings-nav-label">Folders</span>
                                {folders.folders.map((entry) =>
                                    item(
                                        folderPage(entry.path),
                                        folderName(entry.path, name),
                                        <IconFolder size={13} />,
                                        entry.languages.map((language) => LANGUAGE_NAMES[language.id]).join(" · "),
                                    ),
                                )}
                            </div>
                        )}
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
                        {page === "general" && <ProjectGeneralPage project={project} />}
                        {page === "agents" && <ProjectAgentsPage project={project} />}
                        {page === "file" && <ProjectFilePage project={project} />}
                        {folder && folders && (
                            <ProjectFolderPage
                                key={folder.path}
                                project={project}
                                name={name}
                                folder={folder}
                                settings={folders.settings[folder.path]}
                                onChanged={refresh}
                            />
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
}
