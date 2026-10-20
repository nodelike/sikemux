import { folderName, type FolderSettings, type ProjectRules } from "../projects/projectFolders";
import { changeProject, type ProjectChange } from "../projects/projectResources";
import { reportError } from "../state/toast";
import { useProjectTasks, type ProjectTask } from "../tasks/coreTasks";
import { Checkbox, Switch } from "../ui/Controls";
import { Dropdown } from "../ui/Dropdown";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const folderOf = (task: ProjectTask) => (task.cwd === "." ? "." : task.cwd.split("/")[0]);

/** What an agent's work must pass before it says it is finished, chosen per folder from its tasks. */
export function ProjectDonePage({
    project,
    name,
    settings,
    rules,
}: {
    project: string;
    name: string;
    settings: Readonly<Record<string, FolderSettings>>;
    rules: ProjectRules;
}) {
    const tasks = (useProjectTasks(project)?.tasks ?? []).filter((task) => task.source !== "rerun" && task.source !== "command");
    const folders = [...new Set(tasks.map(folderOf))];
    const save = (change: ProjectChange, folder?: string) => void changeProject(project, change, folder).catch(reportError("Save Done means"));

    return (
        <SettingsPage>
            <SettingsSection
                title="Done means"
                sub="The tasks an agent's work must pass before it says it is finished. Agents start them themselves with task_start; tick the ones that prove a change works.">
                {folders.length === 0 ? (
                    <p className="settings-hint">This project has no tasks yet. Add some on the Tasks page first.</p>
                ) : (
                    folders.map((folder) => (
                        <div key={folder} className="project-card">
                            <div className="project-card-head">
                                <span>{folderName(folder, name)}</span>
                                <span className="project-card-count">{settings[folder]?.done?.length ?? 0}</span>
                            </div>
                            {tasks
                                .filter((task) => folderOf(task) === folder)
                                .map((task) => (
                                    <div key={task.id} className="project-done-task">
                                        <Checkbox
                                            checked={settings[folder]?.done?.includes(task.id) ?? false}
                                            onChange={(on) => save({ kind: "done", task: task.id, on }, folder)}>
                                            {task.label}
                                        </Checkbox>
                                        <code className="project-row-command">{task.command}</code>
                                    </div>
                                ))}
                        </div>
                    ))
                )}
            </SettingsSection>
            <SettingsSection title="How agents check">
                <SettingsRows>
                    <SettingsRow
                        asLabel
                        label="Only the folders they changed"
                        desc="An agent that only touched one folder runs that folder's checks."
                        control={<Switch checked={!rules.checkEveryFolder} onChange={(on) => save({ kind: "checkEveryFolder", on: !on })} />}
                    />
                    <SettingsRow label="When a check fails" wide>
                        <Dropdown
                            className="settings-dd"
                            label="When a check fails"
                            value={rules.stopOnFailure ? "stop" : "fix"}
                            options={[
                                { value: "fix", label: "The agent fixes it, then tells you" },
                                { value: "stop", label: "The agent stops and tells you" },
                            ]}
                            onChange={(value) => save({ kind: "stopOnFailure", on: value === "stop" })}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}
