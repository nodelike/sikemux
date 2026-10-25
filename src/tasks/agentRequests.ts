import type { FinishedTestRun } from "./TestResultsCard";
import type { ProjectTask } from "./coreTasks";
import { basename } from "@shared/lib/paths";

function relative(project: string, path: string): string {
    return path === project ? "." : path.startsWith(`${project}/`) ? path.slice(project.length + 1) : path;
}

/** What an agent is asked when the person hands it failing tests: only the failures, and how to check the fix. */
export function testFixRequest(run: FinishedTestRun, project: string): string {
    const failures = run.results.failures.map((failure) => {
        const where = failure.file ? ` (${relative(project, failure.file)}${failure.line ? `:${failure.line}` : ""})` : "";
        return `- ${failure.className} › ${failure.name}: ${failure.message}${where}`;
    });
    return [
        `These tests fail in ${basename(project)} (${run.task.label}):`,
        ...failures,
        `Find the cause and fix it. Then run the task ${run.task.id} with task_start and check testResults in workspace_inspect.`,
    ].join("\n");
}

/** What an agent is asked when a task stopped with an error: it starts the task itself to read why. */
export function failureFixRequest(task: ProjectTask, summary: string, project: string): string {
    return [
        `The task "${task.label}" in ${basename(project)} stopped: ${summary}.`,
        `It runs \`${task.command}\` in ${task.cwd}.`,
        `Start it with task_start (taskId ${task.id}) and read its output with task_read to see the error, then find the cause and fix it.`,
        "Start it again afterwards and check that it keeps running.",
    ].join("\n");
}

/** What an agent is asked to find a project's own run, test and build commands and record them for everyone. */
export function findTasksRequest(project: string): string {
    return [
        `Find how ${basename(project)} is run, tested and built, and save those as Sikemux tasks.`,
        "Read the README, AGENTS.md, Makefile, package.json and any scripts in folders like tool/, bin/ or scripts/.",
        "Then add a task for each command people really use (starting the app, running the tests, building) to sikemux.json at the project root:",
        'each task has an "id", a "label", a "command" and, when it runs in a subfolder, a "cwd". Keep everything already in sikemux.json.',
        "Call guide with the config topic first for the format. Do not start long-running commands; tell me which tasks you added and why.",
    ].join("\n");
}
