import { useState, type ReactNode } from "react";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { folderName, looksSecret, saveFolderSettings, type FolderEnvFile, type FolderSettings, type ProjectFolder } from "../projects/projectFolders";
import { confirmDialog } from "../state/dialog";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { Dropdown } from "../ui/Dropdown";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const NOT_LOADED = "-";
const SHARED_ONLY = "";
const HIDDEN = "••••••••";

interface Shown {
    key: string;
    value: string;
    from: string;
    replaced: boolean;
    own: boolean;
}

/** The files `environment` reads, in the order the core reads them, a later one winning. */
function filesRead(folder: ProjectFolder, environment: string | undefined): FolderEnvFile[] {
    if (environment === undefined) return [];
    const rank = (file: FolderEnvFile) => (file.environment ? 2 : 0) + (file.local ? 1 : 0);
    return folder.envFiles
        .filter((file) => !file.template && (file.environment === null || file.environment === environment))
        .sort((a, b) => rank(a) - rank(b));
}

function shownVariables(read: FolderEnvFile[], own: Readonly<Record<string, string>>): Shown[] {
    const rows: Shown[] = [
        ...read.flatMap((file) => file.variables.map((variable) => ({ ...variable, from: file.path, replaced: false, own: false }))),
        ...Object.entries(own).map(([key, value]) => ({ key, value, from: "set here", replaced: false, own: true })),
    ];
    const last = new Map(rows.map((row, index) => [row.key, index]));
    return rows.map((row, index) => ({ ...row, replaced: last.get(row.key) !== index }));
}

/** The environment processes started in each folder of a project get, one folder at a time. */
export function ProjectEnvironmentPage({
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
    const [picked, setPicked] = useState<string | null>(null);
    const folder = folders.find((entry) => entry.path === picked) ?? folders.find((entry) => entry.envFiles.length) ?? folders[0];
    if (!folder) return null;
    const tabs = folders.length > 1 && (
        <div className="project-folder-tabs" role="tablist" aria-label="Folder">
            {folders.map((entry) => (
                <button
                    key={entry.path}
                    type="button"
                    role="tab"
                    aria-selected={entry === folder}
                    className={entry === folder ? "on" : ""}
                    onClick={() => setPicked(entry.path)}>
                    {folderName(entry.path, name)}
                </button>
            ))}
        </div>
    );
    return (
        <FolderEnvironment
            key={folder.path}
            project={project}
            name={name}
            folder={folder}
            settings={settings[folder.path]}
            onChanged={onChanged}
            above={tabs}
        />
    );
}

function FolderEnvironment({
    project,
    name,
    folder,
    settings,
    onChanged,
    above,
}: {
    project: string;
    name: string;
    folder: ProjectFolder;
    settings: FolderSettings | undefined;
    onChanged: () => void;
    above: ReactNode;
}) {
    const sessionId = useStore((s) => s.activeSessionId);
    const [revealed, setRevealed] = useState(false);
    const [draft, setDraft] = useState({ key: "", value: "" });
    const [asking, setAsking] = useState<{ x: number; y: number } | null>(null);
    const own = settings?.env ?? {};
    const environment = settings?.environment;
    const read = filesRead(folder, environment);
    const rows = shownVariables(read, own);
    const where = folder.path === "." ? "the project root" : `${folder.path}/`;

    const save = (next: FolderSettings) =>
        void saveFolderSettings(project, folder.path, { done: settings?.done, ...next })
            .then(onChanged)
            .catch(reportError("Save the folder's environment"));

    const choose = async (value: string) => {
        if (value === NOT_LOADED) {
            save({ env: own });
            return;
        }
        const production = folder.environments.find((entry) => entry.name === value)?.production;
        if (
            production &&
            !(await confirmDialog({
                title: `Load ${value} in ${name}?`,
                body: "Tasks, terminals and agents started here will get its values, which look like production's. An agent could then reach production systems with them.",
                confirmLabel: `Load ${value}`,
                destructive: true,
            }))
        )
            return;
        save({ environment: value, env: own });
    };

    const addVariable = () => {
        const key = draft.key.trim();
        if (!key) return;
        save({ environment, env: { ...own, [key]: draft.value } });
        setDraft({ key: "", value: "" });
    };

    const removeVariable = (key: string) => {
        const { [key]: _removed, ...rest } = own;
        save({ environment, env: rest });
    };

    return (
        <SettingsPage>
            {above}
            <SettingsSection
                title="Environment"
                meta={folder.envFiles.length ? `${folder.envFiles.length} ${folder.envFiles.length === 1 ? "file" : "files"}` : undefined}
                sub={`Variables tasks, terminals and agents started in ${where} get, also in an agent's worktree of it. What the task or the app sets itself still wins.`}>
                <SettingsRows>
                    <SettingsRow
                        label="Load"
                        desc={
                            environment === undefined
                                ? "Nothing is loaded from the files below."
                                : read.length
                                  ? `Reads ${read.map((file) => file.path).join(", then ")}.`
                                  : "No file belongs to this choice."
                        }
                        wide>
                        <Dropdown
                            className="settings-dd"
                            label="Environment to load"
                            value={environment ?? NOT_LOADED}
                            options={[
                                { value: NOT_LOADED, label: "Don't load" },
                                { value: SHARED_ONLY, label: "Shared files only", detail: "the ones every environment reads" },
                                ...folder.environments.map((entry) => ({
                                    value: entry.name,
                                    label: entry.name,
                                    meta: entry.production ? "production" : undefined,
                                })),
                            ]}
                            onChange={(value) => void choose(value)}
                        />
                    </SettingsRow>
                </SettingsRows>

                {folder.envFiles.length > 0 && (
                    <div className="project-file">
                        <div className="project-file-group-head">
                            <span>Files found</span>
                        </div>
                        {folder.envFiles.map((file) => (
                            <div key={file.path} className={`project-file-item${read.includes(file) ? "" : " unused"}`}>
                                <code className="project-file-command">{file.path}</code>
                                <span className="project-file-name">
                                    {file.template ? "example, never read" : (file.environment ?? "every environment")}
                                    {file.local && !file.template ? " · this machine's" : ""}
                                </span>
                                <span className="project-file-note">
                                    {file.loadedBy.length
                                        ? `loaded by ${file.loadedBy.join(", ")}`
                                        : `${file.variables.length} ${file.variables.length === 1 ? "variable" : "variables"}`}
                                </span>
                            </div>
                        ))}
                    </div>
                )}

                <div className="project-file">
                    <div className="project-file-group-head">
                        <span>Variables</span>
                        <span className="project-file-count">{rows.filter((row) => !row.replaced).length}</span>
                        {rows.some((row) => looksSecret(row.key)) && (
                            <button type="button" className="project-file-add-btn" onClick={() => setRevealed((shown) => !shown)}>
                                {revealed ? "Hide secrets" : "Show secrets"}
                            </button>
                        )}
                    </div>
                    {rows.map((row, index) => (
                        <div key={`${row.from}:${row.key}:${index}`} className={`project-folder-variable${row.replaced ? " replaced" : ""}`}>
                            <code className="project-folder-key">{row.key}</code>
                            <code className="project-file-command">{looksSecret(row.key) && !revealed && row.value ? HIDDEN : row.value}</code>
                            <span className={`project-file-note${row.own ? " own" : ""}`}>{row.from}</span>
                            {row.own ? (
                                <button
                                    type="button"
                                    className="project-folder-remove"
                                    aria-label={`Remove ${row.key}`}
                                    onClick={() => removeVariable(row.key)}>
                                    ×
                                </button>
                            ) : (
                                <span />
                            )}
                        </div>
                    ))}
                    <form
                        className="project-folder-add"
                        onSubmit={(event) => {
                            event.preventDefault();
                            addVariable();
                        }}>
                        <input
                            className="settings-input mono"
                            aria-label="Variable name"
                            placeholder="NAME"
                            spellCheck={false}
                            value={draft.key}
                            onChange={(event) => setDraft({ ...draft, key: event.target.value })}
                        />
                        <input
                            className="settings-input mono"
                            aria-label="Variable value"
                            placeholder="value"
                            spellCheck={false}
                            value={draft.value}
                            onChange={(event) => setDraft({ ...draft, value: event.target.value })}
                        />
                        <button type="submit" className="settings-btn" disabled={!draft.key.trim()}>
                            Add
                        </button>
                    </form>
                    <div className="project-file-foot">
                        <span className="settings-hint">Kept on this host. Sikemux never writes to the project's files.</span>
                        <button
                            type="button"
                            className="settings-btn"
                            onClick={(event) => {
                                const rect = event.currentTarget.getBoundingClientRect();
                                setAsking({ x: rect.right, y: rect.bottom + 4 });
                            }}>
                            Find environments with an agent
                        </button>
                    </div>
                </div>
            </SettingsSection>
            {asking && (
                <SendToAgentMenu
                    x={asking.x}
                    y={asking.y}
                    sessionId={sessionId}
                    alignRight
                    delivery={() => ({ text: environmentsRequest(name, where) })}
                    onClose={() => setAsking(null)}
                />
            )}
        </SettingsPage>
    );
}

function environmentsRequest(project: string, where: string): string {
    return [
        `Find how ${project} gives its processes environment variables in ${where}.`,
        "Look for the files that hold them, whatever they are called (such as .env.stage, .environment.prod or config/dev.env), and for what loads which file:",
        "package.json scripts (dotenv, env-cmd, --env-file), framework modes (Vite --mode, Next.js, Spring profiles), docker-compose env_file, .envrc and Makefile.",
        "Tell me which environments exist, which files each one reads and in what order, and which one local development should use.",
        "Do not change those files and do not print any secret values.",
    ].join("\n");
}
