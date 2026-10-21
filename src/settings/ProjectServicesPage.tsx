import { useState } from "react";
import { coreCall } from "../api/core";
import { folderName, type FolderSettings, type ProjectFolder } from "../projects/projectFolders";
import { changeProject, projectServicesR } from "../projects/projectResources";
import { useResource } from "../state/resources";
import { reportError } from "../state/toast";
import { Checkbox } from "../ui/Controls";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

/** The docker-compose services the project needs, started or stopped here, and which folders' tasks start them first. */
export function ProjectServicesPage({
    project,
    name,
    folders,
    settings,
}: {
    project: string;
    name: string;
    folders: readonly ProjectFolder[];
    settings: Readonly<Record<string, FolderSettings>>;
}) {
    const services = useResource(projectServicesR, project);
    const compose = services.data ?? null;
    const refresh = () => void services.refresh();
    const [busy, setBusy] = useState<string | null>(null);
    const act = (folder: string, service: string, action: "start" | "stop") => {
        setBusy(`${folder}:${service}`);
        void coreCall<void>("serviceAction", { project, folder, service, action })
            .catch(reportError(`${action === "start" ? "Start" : "Stop"} ${service}`))
            .finally(() => {
                setBusy(null);
                refresh();
            });
    };
    const toggle = (folder: string, name: string, on: boolean) =>
        void changeProject(project, { kind: "service", name, on }, folder).catch(reportError("Save the folder's services"));
    const servicesFor = (folder: string) =>
        (compose?.find((entry) => entry.folder === folder) ?? compose?.find((entry) => entry.folder === "."))?.services ?? [];
    const codeFolders = folders.filter((folder) => folder.languages.length && servicesFor(folder.path).length);

    if (compose === null) return <SettingsPage>{null}</SettingsPage>;
    return (
        <SettingsPage>
            <SettingsSection
                title="Services"
                meta={compose.length ? compose.map((entry) => entry.file).join(", ") : undefined}
                sub="What the app needs running beside it, from the project's docker-compose files. Agents can start them as tasks too.">
                {compose.length === 0 ? (
                    <p className="settings-hint">This project has no docker-compose file at its root or in the folders just inside it.</p>
                ) : (
                    compose.map((entry) => (
                        <div key={entry.folder} className="project-card">
                            <div className="project-card-head">
                                <span>{folderName(entry.folder, name)}</span>
                                <span className="project-card-count">{entry.file}</span>
                                <button type="button" className="project-card-action" onClick={refresh}>
                                    Refresh
                                </button>
                            </div>
                            {!entry.docker && (
                                <div className="project-card-status">
                                    Docker did not answer, so whether each one runs is unknown. Start Docker and refresh.
                                </div>
                            )}
                            {entry.services.map((service) => {
                                const running = service.state === "running";
                                const key = `${entry.folder}:${service.name}`;
                                return (
                                    <div key={service.name} className="project-service">
                                        <div className="project-suggestion-text">
                                            <span className="project-row-name">{service.name}</span>
                                            <code className="project-row-command">
                                                {[service.image, ...service.ports].filter(Boolean).join(" · ")}
                                            </code>
                                        </div>
                                        <span className={`project-pill ${running ? "ok" : ""}`}>{service.state ?? "not created"}</span>
                                        <button
                                            type="button"
                                            className="settings-btn"
                                            disabled={busy !== null || !entry.docker}
                                            onClick={() => act(entry.folder, service.name, running ? "stop" : "start")}>
                                            {busy === key ? "…" : running ? "Stop" : "Start"}
                                        </button>
                                    </div>
                                );
                            })}
                        </div>
                    ))
                )}
            </SettingsSection>
            {codeFolders.length > 0 && (
                <SettingsSection
                    title="Started before tasks"
                    sub="Before a task in the folder runs, Sikemux starts what it needs and waits until it is up. Agents read this in the brief.">
                    {codeFolders.map((folder) => (
                        <div key={folder.path} className="project-card">
                            <div className="project-card-head">
                                <span>{folderName(folder.path, name)}</span>
                            </div>
                            <div className="project-service-picks">
                                {servicesFor(folder.path).map((service) => (
                                    <Checkbox
                                        key={service.name}
                                        checked={settings[folder.path]?.services?.includes(service.name) ?? false}
                                        onChange={(on) => toggle(folder.path, service.name, on)}>
                                        {service.name}
                                    </Checkbox>
                                ))}
                            </div>
                        </div>
                    ))}
                </SettingsSection>
            )}
        </SettingsPage>
    );
}
