import { useState, useSyncExternalStore, type ReactNode } from "react";
import { coreCall } from "../api/core";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { projectControllerBridge } from "../projects/controllerBridge";
import { loadProjectConfig } from "../projects/projectConfig";
import { trustProjectConfig } from "../projects/projectConfigRuntime";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { reportError } from "../state/toast";
import { AddTaskForm } from "../tasks/AddTaskForm";
import { findTasksRequest } from "../tasks/agentRequests";
import { useProjectTasks, type ProjectTask } from "../tasks/coreTasks";
import { SettingsPage, SettingsSection } from "./SettingsLayout";

const PLACEMENT_NAMES: Record<string, string> = {
    background: "in the background",
    terminal: "in a terminal",
    split: "in a split",
    popup: "in a popup",
    replace: "in this pane",
};

/** What the project's sikemux.json adds, whether this host trusts it, and what is wrong with it. */
export function ProjectFilePage({ project }: { project: string }) {
    const snapshot = useSyncExternalStore(projectControllerBridge.subscribe, projectControllerBridge.getActiveSnapshot);
    const config = snapshot?.cwd === project ? snapshot.config : null;
    const tasks = useProjectTasks(project);
    const sessionId = useStore((s) => s.activeSessionId);
    const [adding, setAdding] = useState(false);
    const [asking, setAsking] = useState<{ x: number; y: number; text: string } | null>(null);
    const ask = (anchor: HTMLElement, text: string) => {
        const rect = anchor.getBoundingClientRect();
        setAsking({ x: rect.right, y: rect.bottom + 4, text });
    };
    const open = () => cmd.requestOpenFile(config?.path ?? `${project}/sikemux.json`);
    const ours = (tasks?.tasks ?? []).filter((task) => task.source === "project" || task.source === "personal");

    return (
        <SettingsPage>
            <SettingsSection
                title="sikemux.json"
                meta={config?.status === "valid" ? commandCount(ours.length + config.config.actions.length) : undefined}
                sub="The actions, tasks and preview this project adds for everyone who opens it. Commit it to share them with your team.">
                <div className="project-file">
                    {!config || config.status === "absent" ? (
                        <div className="project-file-empty">
                            <p>This project has no sikemux.json yet. Commands added to it show up in the run menu and the command deck.</p>
                            <div className="settings-actions">
                                <button type="button" className="settings-btn" onClick={() => setAdding(true)}>
                                    Add a task
                                </button>
                                <button
                                    type="button"
                                    className="settings-btn primary"
                                    onClick={(event) => ask(event.currentTarget, findTasksRequest(project))}>
                                    Find tasks with an agent
                                </button>
                            </div>
                        </div>
                    ) : config.status === "invalid" ? (
                        <>
                            <div className="project-file-status">
                                <span className="project-file-pill failed">
                                    {config.errors.length} {config.errors.length === 1 ? "problem" : "problems"}
                                </span>
                                <span>Nothing in the file is used until they are fixed.</span>
                            </div>
                            <ul className="project-file-errors">
                                {config.errors.map((error) => (
                                    <li key={`${error.path}:${error.code}`}>
                                        <code>{error.path}</code>
                                        <span>{error.message}</span>
                                    </li>
                                ))}
                            </ul>
                            <div className="project-file-foot">
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
                            <TrustStatus project={project} trusted={tasks?.trusted ?? config.trusted} runsNothing={!config.trust.requiresApproval} />
                            <Group title="Tasks" count={ours.length} add={() => setAdding(true)}>
                                {ours.map((task) => (
                                    <Item key={task.id} name={task.label} command={task.command} note={whoGets(task)} />
                                ))}
                            </Group>
                            <Group title="Actions" count={config.config.actions.length}>
                                {config.config.actions.map((action) => (
                                    <Item key={action.id} name={action.label} command={action.command} note={PLACEMENT_NAMES[action.placement]} />
                                ))}
                            </Group>
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
                            <div className="project-file-foot">
                                <span className="settings-hint">
                                    Tasks marked only me are kept in <code>.sikemux/</code>, which Git ignores.
                                </span>
                                <button type="button" className="settings-btn" onClick={open}>
                                    Open sikemux.json
                                </button>
                            </div>
                        </>
                    )}
                </div>
                {adding && (
                    <div className="project-file-add">
                        <AddTaskForm project={project} onDone={() => setAdding(false)} />
                    </div>
                )}
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

function TrustStatus({ project, trusted, runsNothing }: { project: string; trusted: boolean; runsNothing: boolean }) {
    if (runsNothing) {
        return (
            <div className="project-file-status">
                <span className="project-file-pill">Runs nothing</span>
                <span>It names no commands, so there is nothing to trust.</span>
            </div>
        );
    }
    if (trusted) {
        return (
            <div className="project-file-status">
                <span className="project-file-pill ok">Trusted on this host</span>
                <span>Its commands run without asking until the file changes.</span>
                <button
                    type="button"
                    className="settings-btn"
                    onClick={() => void coreCall("untrustProject", { project }).catch(reportError("Stop trusting sikemux.json"))}>
                    Stop trusting
                </button>
            </div>
        );
    }
    return (
        <div className="project-file-status">
            <span className="project-file-pill waiting">Not trusted yet</span>
            <span>Nothing below runs until you look at its commands and trust them.</span>
            <button
                type="button"
                className="settings-btn primary"
                onClick={() =>
                    void loadProjectConfig(project)
                        .then((fresh) => (fresh.status === "valid" ? trustProjectConfig(fresh) : false))
                        .catch(reportError("Trust sikemux.json"))
                }>
                Review and trust…
            </button>
        </div>
    );
}

function Group({ title, count, add, children }: { title: string; count?: number; add?: () => void; children: ReactNode }) {
    return (
        <div className="project-file-group">
            <div className="project-file-group-head">
                <span>{title}</span>
                {count !== undefined && <span className="project-file-count">{count}</span>}
                {add && (
                    <button type="button" className="project-file-add-btn" onClick={add}>
                        + Add a task
                    </button>
                )}
            </div>
            {children}
        </div>
    );
}

function Item({ name, command, note }: { name: string; command: string; note?: string }) {
    return (
        <div className="project-file-item">
            <span className="project-file-name">{name}</span>
            <code className="project-file-command">{command}</code>
            <span className="project-file-note">{note}</span>
        </div>
    );
}

function whoGets(task: ProjectTask): string {
    return task.source === "personal" ? "only me" : "team";
}

function fixRequest(path: string, errors: readonly { path: string; message: string }[]): string {
    return [
        `${path} has problems, so Sikemux uses nothing in it:`,
        ...errors.map((error) => `- ${error.path}: ${error.message}`),
        "Fix them and keep everything else in the file. Call guide with the config topic first for the format.",
    ].join("\n");
}

function commandCount(count: number): string {
    return `${count} ${count === 1 ? "command" : "commands"}`;
}
