import type { CustomCommand } from "./commands/registry";
import type { ProjectAction, ProjectConfigLoadResult, ProjectTask, ProjectWorktreeCreateHook, SikemuxProjectConfig } from "./projectConfig";
import { confirmDialog, type ConfirmRequest } from "./state/dialog";

const trustedConfigs = new Set<string>();

function trustKey(result: Extract<ProjectConfigLoadResult, { status: "valid" }>): string {
    return `${result.path}\0${result.fingerprint}`;
}

export function projectActionCommand(action: ProjectAction): CustomCommand {
    return {
        id: `project.${action.id}`,
        title: action.label,
        detail: action.description,
        command: action.command,
        contexts: action.contexts,
        placement: action.placement,
    };
}

export function worktreeHookCommand(hook: ProjectWorktreeCreateHook): CustomCommand {
    return {
        id: `project.worktree.${hook.id}`,
        title: hook.label,
        detail: "Project-defined worktree setup",
        command: hook.command,
        contexts: ["project"],
        placement: "background",
    };
}

function taskCommandLine(task: ProjectTask): string {
    const cd = task.cwd === "." ? [] : [`cd ${task.cwd} &&`];
    const env = Object.entries(task.env).map(([name, value]) => `${name}=${value}`);
    return [...cd, ...env, task.command].join(" ");
}

function configCommands(config: SikemuxProjectConfig): { label: string; command: string }[] {
    return [
        ...config.actions.map((action) => ({ label: `Action · ${action.label}`, command: action.command })),
        ...config.tasks.map((task) => ({ label: `Task · ${task.label}`, command: taskCommandLine(task) })),
        ...(config.preview?.command ? [{ label: "Preview", command: config.preview.command }] : []),
        ...(config.worktree?.onCreate ?? []).map((hook) => ({ label: `New worktree · ${hook.label}`, command: hook.command })),
    ];
}

/**
 * Already-trusted configs resolve synchronously on the microtask queue; only a
 * first-time approval reaches the dialog.
 */
export async function trustProjectConfig(
    result: Extract<ProjectConfigLoadResult, { status: "valid" }>,
    ask: (request: ConfirmRequest) => Promise<boolean> = confirmDialog,
): Promise<boolean> {
    const key = trustKey(result);
    if (!result.trust.requiresApproval || trustedConfigs.has(key)) return true;
    const approved = await ask({
        title: "Trust this sikemux.json?",
        body: "It lets Sikemux run these commands in this project. Trust lasts until Sikemux closes or the file changes.",
        commands: configCommands(result.config),
        confirmLabel: "Trust project",
    });
    if (approved) trustedConfigs.add(key);
    return approved;
}

export function clearProjectConfigTrustForTests(): void {
    trustedConfigs.clear();
}
