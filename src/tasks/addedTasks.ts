import { fsapi } from "../api/fs";
import { joinPath } from "../lib/paths";
import { PROJECT_CONFIG_FILE } from "../projects/projectConfig";
import { confirmDialog, type ConfirmRequest } from "../state/dialog";
import type { TaskDefinitionInput } from "./taskRegistry";

/** Tasks one person added for themselves, in a folder that keeps itself out of Git. */
export const PERSONAL_TASKS = ".sikemux/tasks.json";
export const PERSONAL_TASK_PREFIX = "personal.";

export interface AddedTask {
    readonly label: string;
    readonly command: string;
    /** Relative to the project, "." for its root. */
    readonly cwd: string;
}

interface StoredTask {
    id: string;
    label: string;
    command: string;
    cwd: string;
}

export function taskIdFor(label: string): string {
    const slug = label
        .toLowerCase()
        .replace(/[^a-z0-9._-]+/g, "-")
        .replace(/^-+|-+$/g, "")
        .slice(0, 60);
    return slug || "task";
}

function isStoredTask(value: unknown): value is StoredTask {
    const task = value as Partial<StoredTask> | null;
    return typeof task?.id === "string" && typeof task.label === "string" && typeof task.command === "string" && typeof task.cwd === "string";
}

async function readJson(path: string): Promise<Record<string, unknown> | null> {
    const text = await fsapi.readTextFileLimited(path).catch(() => null);
    if (text === null) return null;
    const value = JSON.parse(text) as unknown;
    if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error(`${path} is not a JSON object`);
    return value as Record<string, unknown>;
}

/** Every task already there stays as it was, except one with the same id, which the new one replaces. */
function withTask(existing: unknown, task: Partial<StoredTask>): unknown[] {
    const tasks: unknown[] = Array.isArray(existing) ? existing : [];
    return [...tasks.filter((candidate) => (candidate as { id?: unknown } | null)?.id !== task.id), task];
}

interface PersonalTaskRun {
    readonly project: string;
    readonly label: string;
    readonly command: string;
    readonly cwd: string;
}

/** Personal tasks this person saved or approved since Sikemux opened. */
const trustedPersonalTasks = new Set<string>();

const trustKey = (task: Omit<PersonalTaskRun, "label">) => `${task.project}\0${task.cwd}\0${task.command}`;

const absoluteCwd = (project: string, cwd: string) => (cwd === "." ? project : joinPath(project, cwd));

/** A cloned repository can ship its own .sikemux/tasks.json, so a task saved there runs only once the person has seen its command. */
export async function trustPersonalTask(task: PersonalTaskRun, ask: (request: ConfirmRequest) => Promise<boolean> = confirmDialog): Promise<boolean> {
    const key = trustKey(task);
    if (trustedPersonalTasks.has(key)) return true;
    const approved = await ask({
        title: `Run ${task.label}?`,
        body: "This task comes from the project's .sikemux/tasks.json. Trust lasts until Sikemux closes or the task changes.",
        commands: [{ label: task.label, command: task.cwd === task.project ? task.command : `cd ${task.cwd} && ${task.command}` }],
        confirmLabel: "Run task",
    });
    if (approved) trustedPersonalTasks.add(key);
    return approved;
}

export async function readPersonalTasks(project: string): Promise<TaskDefinitionInput[]> {
    const stored = await readJson(joinPath(project, PERSONAL_TASKS)).catch(() => null);
    const tasks = Array.isArray(stored?.tasks) ? stored.tasks.filter(isStoredTask) : [];
    return tasks.map((task) => ({
        id: `${PERSONAL_TASK_PREFIX}${task.id}`,
        label: task.label,
        command: task.command,
        project,
        cwd: absoluteCwd(project, task.cwd),
    }));
}

/** Saves a task for this person only; returns its task id. */
export async function savePersonalTask(project: string, task: AddedTask): Promise<string> {
    const folder = joinPath(project, ".sikemux");
    const [kind] = await fsapi.pathKinds([folder]);
    if (kind !== "dir") await fsapi.createDir(folder);
    await fsapi.writeFile(joinPath(folder, ".gitignore"), "*\n");
    const path = joinPath(project, PERSONAL_TASKS);
    const stored = (await readJson(path)) ?? {};
    const id = taskIdFor(task.label);
    await fsapi.writeFile(path, `${JSON.stringify({ ...stored, tasks: withTask(stored.tasks, { id, ...task }) }, null, 2)}\n`);
    trustedPersonalTasks.add(trustKey({ project, cwd: absoluteCwd(project, task.cwd), command: task.command }));
    return `${PERSONAL_TASK_PREFIX}${id}`;
}

/** Adds a task to the project's sikemux.json, keeping everything else in it; returns its task id. */
export async function saveTeamTask(project: string, task: AddedTask): Promise<string> {
    const path = joinPath(project, PROJECT_CONFIG_FILE);
    const config = (await readJson(path)) ?? { version: 1 };
    const id = taskIdFor(task.label);
    const entry = { id, label: task.label, command: task.command, ...(task.cwd === "." ? {} : { cwd: task.cwd }) };
    await fsapi.writeFile(path, `${JSON.stringify({ ...config, tasks: withTask(config.tasks, entry) }, null, 2)}\n`);
    return id;
}
