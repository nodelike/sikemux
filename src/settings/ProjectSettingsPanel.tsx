import { useEffect, type ReactNode } from "react";
import { prettyPath } from "../lib/paths";
import * as cmd from "../state/commands";
import { useStore, type StoreState } from "../state/store";
import { IconAgent, IconClose, IconFolder } from "../ui/Icons";
import { PaneField } from "../ui/ShaderField";
import { useLeaveOnNavigation } from "./leaveSettings";
import { ProjectAgentsPage, ProjectGeneralPage } from "./ProjectPage";
import { PROJECT_SETTINGS_PAGES, type ProjectSettingsPageId } from "./projectSettingsPages";
import "../styles/settings.css";

const PAGE_ICONS: Record<ProjectSettingsPageId, ReactNode> = {
    general: <IconFolder size={13} />,
    agents: <IconAgent size={13} />,
};

const isOpen = (state: StoreState) => state.projectSettingsPage !== null;

/** The project in front's own settings, standing in for its workspace as the app's settings do. */
export function ProjectSettingsPanel() {
    const page = useStore((s) => s.projectSettingsPage) ?? "general";
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const home = useStore((s) => s.home);
    const paneShader = useStore((s) => s.paneShader);

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
    const project = session.cwd;
    const name = PROJECT_SETTINGS_PAGES.find((entry) => entry.id === page)?.name;

    return (
        <div tabIndex={-1} className="settings-pane" role="dialog" aria-label={`${session.name} settings`}>
            <div className="settings-frame">
                <aside className="settings-rail">
                    <div className="project-settings-head">
                        <IconFolder size={13} />
                        <span className="project-settings-name">{session.name}</span>
                        <span className="project-settings-path">{prettyPath(project, home)}</span>
                    </div>
                    <nav className="settings-nav" aria-label="Project settings sections">
                        <div className="settings-nav-group">
                            {PROJECT_SETTINGS_PAGES.map((entry) => (
                                <button
                                    key={entry.id}
                                    type="button"
                                    className={`settings-rail-item${page === entry.id ? " active" : ""}`}
                                    aria-current={page === entry.id ? "page" : undefined}
                                    onClick={() => cmd.openProjectSettings(entry.id)}>
                                    <span className="settings-rail-icon" aria-hidden="true">
                                        {PAGE_ICONS[entry.id]}
                                    </span>
                                    <span className="settings-rail-name">{entry.name}</span>
                                </button>
                            ))}
                        </div>
                    </nav>
                    <div className="settings-rail-foot">Kept on this host, not in the project</div>
                </aside>

                <div className="settings-main">
                    <PaneField enabled={paneShader} />
                    <header className="settings-topbar">
                        <span className="settings-topbar-title">{name}</span>
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
                    </div>
                </div>
            </div>
        </div>
    );
}
