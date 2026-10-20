import { useEffect, useState } from "react";
import {
    folderName,
    loadProjectBrief,
    saveFolderSettings,
    saveProjectRules,
    type FolderSettings,
    type ProjectFolder,
    type ProjectRules,
} from "../projects/projectFolders";
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
    onChanged,
}: {
    project: string;
    name: string;
    settings: Readonly<Record<string, FolderSettings>>;
    rules: ProjectRules;
    onChanged: () => void;
}) {
    const tasks = (useProjectTasks(project)?.tasks ?? []).filter((task) => task.source !== "rerun" && task.source !== "command");
    const folders = [...new Set(tasks.map(folderOf))];

    const toggle = (folder: string, id: string, on: boolean) => {
        const current = settings[folder] ?? {};
        const done = on ? [...(current.done ?? []), id] : (current.done ?? []).filter((kept) => kept !== id);
        void saveFolderSettings(project, folder, { ...current, done })
            .then(onChanged)
            .catch(reportError("Save Done means"));
    };
    const setRules = (change: ProjectRules) =>
        void saveProjectRules(project, { ...rules, ...change })
            .then(onChanged)
            .catch(reportError("Save Done means"));

    return (
        <SettingsPage>
            <SettingsSection
                title="Done means"
                sub="The tasks an agent's work must pass before it says it is finished. Agents start them themselves with task_start; tick the ones that prove a change works.">
                {folders.length === 0 ? (
                    <p className="settings-hint">This project has no tasks yet. Add some on the Tasks page first.</p>
                ) : (
                    folders.map((folder) => (
                        <div key={folder} className="project-file">
                            <div className="project-file-group-head">
                                <span>{folderName(folder, name)}</span>
                                <span className="project-file-count">{settings[folder]?.done?.length ?? 0}</span>
                            </div>
                            {tasks
                                .filter((task) => folderOf(task) === folder)
                                .map((task) => (
                                    <div key={task.id} className="project-done-task">
                                        <Checkbox
                                            checked={settings[folder]?.done?.includes(task.id) ?? false}
                                            onChange={(on) => toggle(folder, task.id, on)}>
                                            {task.label}
                                        </Checkbox>
                                        <code className="project-file-command">{task.command}</code>
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
                        control={<Switch checked={!rules.checkEveryFolder} onChange={(on) => setRules({ checkEveryFolder: !on })} />}
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
                            onChange={(value) => setRules({ stopOnFailure: value === "stop" })}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}

/** The brief every agent working in the project reads, written from its settings, with the person's own notes. */
export function ProjectBriefPage({
    project,
    folders,
    settings,
    rules,
    onChanged,
}: {
    project: string;
    folders: readonly ProjectFolder[];
    settings: Readonly<Record<string, FolderSettings>>;
    rules: ProjectRules;
    onChanged: () => void;
}) {
    const [brief, setBrief] = useState<string | null>(null);
    const [draft, setDraft] = useState<string | null>(null);
    const notes = rules.notes ?? "";
    useEffect(() => {
        let cancelled = false;
        void loadProjectBrief(project)
            .then((text) => {
                if (!cancelled) setBrief(text);
            })
            .catch(() => {});
        return () => {
            cancelled = true;
        };
    }, [project, folders, settings, rules]);

    const saveNotes = () => {
        const next = draft?.trim();
        setDraft(null);
        if (next === undefined || next === notes) return;
        void saveProjectRules(project, { ...rules, notes: next })
            .then(onChanged)
            .catch(reportError("Save notes for agents"));
    };

    return (
        <SettingsPage>
            <SettingsSection
                title="What agents know"
                sub="Every agent working here reads this in workspace_inspect before it builds, runs or tests anything, so it uses your JDK, profiles and tasks instead of guessing. Secret values are never in it.">
                <div className="project-file">
                    <pre className="project-brief">
                        {brief === null ? "…" : brief || "Nothing set up yet. Settings on the other pages show up here."}
                    </pre>
                </div>
            </SettingsSection>
            <SettingsSection
                title="Your notes for agents"
                sub="Added to the end of the brief. Agents also read the project's own AGENTS.md or CLAUDE.md.">
                <textarea
                    className="settings-input project-notes"
                    aria-label="Notes for agents"
                    placeholder="Use the money helpers in api/util/Money.java; never call Stripe from tests."
                    value={draft ?? notes}
                    onChange={(event) => setDraft(event.target.value)}
                    onBlur={saveNotes}
                />
            </SettingsSection>
        </SettingsPage>
    );
}
