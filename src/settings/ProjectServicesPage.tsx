import { useEffect, useState } from "react";
import { coreCall } from "../api/core";
import { folderName, saveFolderSettings, type FolderSettings, type ProjectFolder } from "../projects/projectFolders";
import { reportError } from "../state/toast";
import { Checkbox } from "../ui/Controls";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

interface Service {
    name: string;
    image?: string;
    ports: string[];
    state?: string;
}

interface ComposeFolder {
    folder: string;
    file: string;
    services: Service[];
    docker: boolean;
}

function useServices(project: string): { compose: ComposeFolder[] | null; refresh: () => void } {
    const [compose, setCompose] = useState<ComposeFolder[] | null>(null);
    const [version, setVersion] = useState(0);
    useEffect(() => {
        let cancelled = false;
        void coreCall<ComposeFolder[]>("projectServices", { project })
            .then((found) => {
                if (!cancelled) setCompose(found);
            })
            .catch(() => setCompose([]));
        return () => {
            cancelled = true;
        };
    }, [project, version]);
    return { compose, refresh: () => setVersion((current) => current + 1) };
}

/** The docker-compose services the project needs, started or stopped here, and which folders' tasks start them first. */
export function ProjectServicesPage({
    project,
    name,
    folders,
    settings,
    onChanged,
}: {
    project: string;
    name: string;
    folders: readonly ProjectFolder[];
    settings: Readonly<Record<string, FolderSettings>>;
    onChanged: () => void;
}) {
    const { compose, refresh } = useServices(project);
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
    const toggle = (folder: string, service: string, on: boolean) => {
        const current = settings[folder] ?? {};
        const services = on ? [...(current.services ?? []), service] : (current.services ?? []).filter((kept) => kept !== service);
        void saveFolderSettings(project, folder, { ...current, services })
            .then(onChanged)
            .catch(reportError("Save the folder's services"));
    };
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
                        <div key={entry.folder} className="project-file">
                            <div className="project-file-group-head">
                                <span>{folderName(entry.folder, name)}</span>
                                <span className="project-file-count">{entry.file}</span>
                                <button type="button" className="project-file-add-btn" onClick={refresh}>
                                    Refresh
                                </button>
                            </div>
                            {!entry.docker && (
                                <div className="project-file-status">
                                    Docker did not answer, so whether each one runs is unknown. Start Docker and refresh.
                                </div>
                            )}
                            {entry.services.map((service) => {
                                const running = service.state === "running";
                                const key = `${entry.folder}:${service.name}`;
                                return (
                                    <div key={service.name} className="project-service">
                                        <div className="project-suggestion-text">
                                            <span className="project-file-name">{service.name}</span>
                                            <code className="project-file-command">
                                                {[service.image, ...service.ports].filter(Boolean).join(" · ")}
                                            </code>
                                        </div>
                                        <span className={`project-file-pill ${running ? "ok" : ""}`}>{service.state ?? "not created"}</span>
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
                        <div key={folder.path} className="project-file">
                            <div className="project-file-group-head">
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
