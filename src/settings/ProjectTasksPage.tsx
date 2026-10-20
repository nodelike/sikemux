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

/** Everything the project can run, and what its sikemux.json adds, whether this host trusts it, and what is wrong with it. */
export function ProjectTasksPage({ project }: { project: string }) {
    const snapshot = useSyncExternalStore(projectControllerBridge.subscribe, projectControllerBridge.getActiveSnapshot);
    const config = snapshot?.cwd === project ? snapshot.config : null;
    const found = useProjectTasks(project);
    const tasks = found?.tasks ?? [];
    const trusted = found?.trusted;
    const sessionId = useStore((s) => s.activeSessionId);
    const [adding, setAdding] = useState(false);
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
                <div className="project-file">
                    {listed.length ? (
                        listed.map((task) => <Item key={task.id} name={task.label} command={task.command} note={whereFrom(task)} />)
                    ) : (
                        <div className="project-file-empty">
                            <p>Nothing to run yet: no task in sikemux.json, and none found in the build files.</p>
                        </div>
                    )}
                    <div className="project-file-foot">
                        <span />
                        <button type="button" className="settings-btn" onClick={(event) => ask(event.currentTarget, findTasksRequest(project))}>
                            Find tasks with an agent
                        </button>
                        <button type="button" className="settings-btn primary" onClick={() => setAdding(true)}>
                            Add a task
                        </button>
                    </div>
                </div>
                {adding && (
                    <div className="project-file-add">
                        <AddTaskForm project={project} onDone={() => setAdding(false)} />
                    </div>
                )}
            </SettingsSection>

            <SettingsSection title="sikemux.json" sub="The actions, preview and worktree commands this project adds for everyone who opens it.">
                <div className="project-file">
                    {!config || config.status === "absent" ? (
                        <div className="project-file-empty">
                            <p>This project has no sikemux.json. Adding a task for the team makes one.</p>
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
                            <TrustStatus project={project} trusted={trusted ?? config.trusted} runsNothing={!config.trust.requiresApproval} />
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
                            <div className="project-file-foot">
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

function Group({ title, count, children }: { title: string; count?: number; children: ReactNode }) {
    return (
        <div className="project-file-group">
            <div className="project-file-group-head">
                <span>{title}</span>
                {count !== undefined && <span className="project-file-count">{count}</span>}
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
