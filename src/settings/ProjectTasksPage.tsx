import { useEffect, useState, type ReactNode } from "react";
import { coreCall } from "../api/core";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { loadProjectConfig, type ProjectConfigLoadResult } from "../projects/projectConfig";
import { trustProjectConfig } from "../projects/projectConfigRuntime";
import * as cmd from "../state/commands";
import { confirmDialog } from "../state/dialog";
import { useStore } from "../state/store";
import { notify, reportError } from "../state/toast";
import { AddTaskDialog } from "../tasks/AddTaskDialog";
import { findTasksRequest } from "../tasks/agentRequests";
import { tasksApi, useProjectTasks, type ProjectTask } from "../tasks/coreTasks";
import { runProjectTask } from "../tasks/projectTaskActions";
import { IconPencil, IconRun, IconTrash } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { SettingsPage, SettingsRow, SettingsRows, SettingsSection } from "./SettingsLayout";

const PLACEMENT_NAMES: Record<string, string> = {
    background: "in the background",
    terminal: "in a terminal",
    split: "in a split",
    popup: "in a popup",
    replace: "in this pane",
};

/** Everything the project can run, and what its sikemux.json adds, whether this host trusts it, and what is wrong with it. */
export function ProjectTasksPage({ project }: { project: string }) {
    const found = useProjectTasks(project);
    const [config, setConfig] = useState<ProjectConfigLoadResult | null>(null);
    const reload = () => void loadProjectConfig(project).then(setConfig, reportError("Read sikemux.json"));
    useEffect(() => {
        let cancelled = false;
        void loadProjectConfig(project).then(
            (next) => !cancelled && setConfig(next),
            () => {},
        );
        return () => {
            cancelled = true;
        };
    }, [project, found]);
    const tasks = found?.tasks ?? [];
    const trusted = found?.trusted;
    const sessionId = useStore((s) => s.activeSessionId);
    const [dialog, setDialog] = useState<{ editing?: ProjectTask } | null>(null);
    const [asking, setAsking] = useState<{ x: number; y: number; text: string } | null>(null);
    const ask = (anchor: HTMLElement, text: string) => {
        const rect = anchor.getBoundingClientRect();
        setAsking({ x: rect.right, y: rect.bottom + 4, text });
    };
    const open = () => cmd.requestOpenFile(config?.path ?? `${project}/sikemux.json`);
    const listed = tasks.filter((task) => task.source !== "rerun" && task.source !== "command");

    return (
        <SettingsPage>
            <SettingsSection
                title="Tasks"
                meta={listed.length ? `${listed.length} ${listed.length === 1 ? "task" : "tasks"}` : undefined}
                sub="What the run menu offers and agents can start. Tasks for the team go in sikemux.json; tasks only you get stay in .sikemux/, which Git ignores.">
                <div className="project-card">
                    {listed.length ? (
                        listed.map((task) =>
                            editable(task) ? (
                                <TaskRow key={task.id} task={task} project={project} onEdit={() => setDialog({ editing: task })} />
                            ) : (
                                <Item key={task.id} name={task.label} command={task.command} note={whereFrom(task)} />
                            ),
                        )
                    ) : (
                        <div className="project-card-empty">
                            <p>Nothing to run yet: no task in sikemux.json, and none found in the build files.</p>
                        </div>
                    )}
                    <div className="project-card-foot">
                        <span />
                        <button type="button" className="settings-btn" onClick={(event) => ask(event.currentTarget, findTasksRequest(project))}>
                            Find tasks with an agent
                        </button>
                        <button type="button" className="settings-btn primary" onClick={() => setDialog({})}>
                            Add a task
                        </button>
                    </div>
                </div>
                {dialog && <AddTaskDialog project={project} editing={dialog.editing} onDone={() => setDialog(null)} />}
            </SettingsSection>

            <CheckSection project={project} config={config} onChanged={reload} />

            <SettingsSection title="sikemux.json" sub="The actions, preview and worktree commands this project adds for everyone who opens it.">
                <div className="project-card">
                    {!config || config.status === "absent" ? (
                        <div className="project-card-empty">
                            <p>This project has no sikemux.json. Adding a task for the team makes one.</p>
                        </div>
                    ) : config.status === "invalid" ? (
                        <>
                            <div className="project-card-status">
                                <span className="project-pill failed">
                                    {config.errors.length} {config.errors.length === 1 ? "problem" : "problems"}
                                </span>
                                <span>Nothing in the file is used until they are fixed.</span>
                            </div>
                            <ul className="project-card-errors">
                                {config.errors.map((error) => (
                                    <li key={`${error.path}:${error.code}`}>
                                        <code>{error.path}</code>
                                        <span>{error.message}</span>
                                    </li>
                                ))}
                            </ul>
                            <div className="project-card-foot">
                                <span />
                                <button
                                    type="button"
                                    className="settings-btn"
                                    onClick={(event) => ask(event.currentTarget, fixRequest(config.path, config.errors))}>
                                    Ask an agent to fix it
                                </button>
                                <button type="button" className="settings-btn primary" onClick={open}>
                                    Open sikemux.json
                                </button>
                            </div>
                        </>
                    ) : (
                        <>
                            <TrustStatus
                                project={project}
                                trusted={trusted ?? config.trusted}
                                runsNothing={!config.trust.requiresApproval}
                                onChanged={reload}
                            />
                            {config.config.actions.length > 0 && (
                                <Group title="Actions" count={config.config.actions.length}>
                                    {config.config.actions.map((action) => (
                                        <Item key={action.id} name={action.label} command={action.command} note={PLACEMENT_NAMES[action.placement]} />
                                    ))}
                                </Group>
                            )}
                            {config.config.preview?.url && (
                                <Group title="Preview">
                                    <Item name={config.config.preview.url} command={config.config.preview.command ?? ""} />
                                </Group>
                            )}
                            {config.config.worktree?.onCreate.length ? (
                                <Group title="When a worktree is made" count={config.config.worktree.onCreate.length}>
                                    {config.config.worktree.onCreate.map((hook) => (
                                        <Item key={hook.id} name={hook.label} command={hook.command} />
                                    ))}
                                </Group>
                            ) : null}
                            {config.config.generate && (
                                <Group title="Generated files">
                                    <Item name={config.config.generate.command} command={config.config.generate.outputs.join(", ")} />
                                </Group>
                            )}
                            <div className="project-card-foot">
                                <span />
                                <button type="button" className="settings-btn" onClick={open}>
                                    Open sikemux.json
                                </button>
                            </div>
                        </>
                    )}
                </div>
            </SettingsSection>
            {asking && (
                <SendToAgentMenu
                    x={asking.x}
                    y={asking.y}
                    sessionId={sessionId}
                    alignRight
                    delivery={() => ({ text: asking.text })}
                    onClose={() => setAsking(null)}
                />
            )}
        </SettingsPage>
    );
}

function TrustStatus({
    project,
    trusted,
    runsNothing,
    onChanged,
}: {
    project: string;
    trusted: boolean;
    runsNothing: boolean;
    onChanged: () => void;
}) {
    if (runsNothing) {
        return (
            <div className="project-card-status">
                <span className="project-pill">Runs nothing</span>
                <span>It names no commands, so there is nothing to trust.</span>
            </div>
        );
    }
    if (trusted) {
        return (
            <div className="project-card-status">
                <span className="project-pill ok">Trusted on this host</span>
                <span>Its commands run without asking until the file changes.</span>
                <button
                    type="button"
                    className="settings-btn"
                    onClick={() => void coreCall("untrustProject", { project }).then(onChanged, reportError("Stop trusting sikemux.json"))}>
                    Stop trusting
                </button>
            </div>
        );
    }
    return (
        <div className="project-card-status">
            <span className="project-pill waiting">Not trusted yet</span>
            <span>Nothing below runs until you look at its commands and trust them.</span>
            <button
                type="button"
                className="settings-btn primary"
                onClick={() =>
                    void loadProjectConfig(project)
                        .then((fresh) => (fresh.status === "valid" ? trustProjectConfig(fresh) : false))
                        .then((trustedNow) => trustedNow && onChanged(), reportError("Trust sikemux.json"))
                }>
                Review and trust…
            </button>
        </div>
    );
}

function Group({ title, count, children }: { title: string; count?: number; children: ReactNode }) {
    return (
        <div className="project-card-group">
            <div className="project-card-head">
                <span>{title}</span>
                {count !== undefined && <span className="project-card-count">{count}</span>}
            </div>
            {children}
        </div>
    );
}

function Item({ name, command, note }: { name: string; command: string; note?: string }) {
    return (
        <div className="project-row">
            <span className="project-row-name">{name}</span>
            <code className="project-row-command">{command}</code>
            <span className="project-row-note">{note}</span>
        </div>
    );
}

const editable = (task: ProjectTask) => task.source === "project" || task.source === "personal";

function TaskRow({ task, project, onEdit }: { task: ProjectTask; project: string; onEdit: () => void }) {
    const remove = async () => {
        const ok = await confirmDialog({
            title: `Remove ${task.label}?`,
            body:
                task.source === "project"
                    ? "It comes out of sikemux.json; commit that to remove it for the team."
                    : "It comes out of .sikemux/tasks.json.",
            confirmLabel: "Remove",
            destructive: true,
        });
        if (!ok) return;
        void tasksApi
            .remove(project, task.id)
            .then(() => notify("success", `Removed ${task.label}`))
            .catch(reportError("remove task"));
    };
    return (
        <div className="project-task-row">
            <span className="project-task-copy">
                <span className="project-task-label">{task.label}</span>
                <span className="project-task-command">{task.command}</span>
            </span>
            <span className="project-task-meta">
                {task.cwd !== "." && <span className="project-task-cwd">{task.cwd}</span>}
                <span>{whereFrom(task)}</span>
            </span>
            <span className="project-task-actions">
                <Tooltip label={`Run ${task.label}`}>
                    <button
                        type="button"
                        className="settings-row-x"
                        aria-label={`Run ${task.label}`}
                        onClick={() => void runProjectTask(project, task)}>
                        <IconRun size={11} />
                    </button>
                </Tooltip>
                <Tooltip label="Edit">
                    <button type="button" className="settings-row-x" aria-label={`Edit ${task.label}`} onClick={onEdit}>
                        <IconPencil size={11} />
                    </button>
                </Tooltip>
                <Tooltip label="Remove">
                    <button type="button" className="settings-row-x" aria-label={`Remove ${task.label}`} onClick={() => void remove()}>
                        <IconTrash size={11} />
                    </button>
                </Tooltip>
            </span>
        </div>
    );
}

function CheckSection({ project, config, onChanged }: { project: string; config: ProjectConfigLoadResult | null; onChanged: () => void }) {
    const saved = config?.status === "valid" ? (config.config.check ?? "") : "";
    const [draft, setDraft] = useState<string | null>(null);
    const save = () => {
        const next = (draft ?? saved).trim();
        setDraft(null);
        if (next === saved) return;
        void tasksApi.setCheck(project, next).then(onChanged, reportError("save the check command"));
    };
    return (
        <SettingsSection title="Check" sub="Sikemux runs it on each commit, and agents run it on a card's change when its pull request has no CI.">
            <SettingsRows>
                <SettingsRow label="Check command" desc="Saved in sikemux.json for everyone. Leave it empty for none." wide>
                    <input
                        className="settings-input mono"
                        value={draft ?? saved}
                        placeholder="pnpm check"
                        aria-label="Check command"
                        spellCheck={false}
                        disabled={config?.status === "invalid"}
                        onChange={(event) => setDraft(event.target.value)}
                        onBlur={save}
                        onKeyDown={(event) => {
                            if (event.key === "Enter") event.currentTarget.blur();
                        }}
                    />
                </SettingsRow>
            </SettingsRows>
        </SettingsSection>
    );
}

function whereFrom(task: ProjectTask): string {
    if (task.source === "personal") return "only me";
    if (task.source === "project") return "sikemux.json";
    return task.cwd === "." ? "found in the build files" : `found in ${task.cwd}/`;
}

function fixRequest(path: string, errors: readonly { path: string; message: string }[]): string {
    return [
        `${path} has problems, so Sikemux uses nothing in it:`,
        ...errors.map((error) => `- ${error.path}: ${error.message}`),
        "Fix them and keep everything else in the file. Call guide with the config topic first for the format.",
    ].join("\n");
}
