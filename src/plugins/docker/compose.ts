import { errorText } from "../../plugin-api/format";
import { docker } from "./resources";
import { confirmDialog, notify, runInTerminal, swallow } from "../../plugin-api/host";
import { dockerApi, type Stack, type StackAction } from "./api";

/** `docker compose` pointed at the stack's own folder and files, as it was brought up. */
export function composeArgv(stack: Pick<Stack, "project" | "workingDir" | "configFiles">, ...command: string[]): string[] {
    const argv = ["docker", "compose", "-p", stack.project];
    if (stack.workingDir) argv.push("--project-directory", stack.workingDir);
    for (const file of stack.configFiles) argv.push("-f", file);
    return [...argv, ...command];
}

/** Runs Compose in a terminal, so its build output and errors show as they happen. */
export function runCompose(stack: Stack, title: string, ...command: string[]): void {
    void runInTerminal(`${title} ${stack.project}`, composeArgv(stack, ...command))
        .then(() => docker.refresh())
        .catch(swallow("run docker compose"));
}

const DONE: Record<StackAction, string> = { start: "Started", stop: "Stopped", restart: "Restarted", down: "Took down" };

export async function actOnStack(stack: Stack, action: StackAction): Promise<void> {
    if (action === "down") {
        const sure = await confirmDialog({
            title: `Take down ${stack.project}?`,
            body: `Its ${stack.total} containers are stopped and removed. Its volumes and images stay, so Up brings it back.`,
            confirmLabel: "Take down",
            destructive: true,
        });
        if (!sure) return;
    }
    try {
        const result = await dockerApi.actOnStack(stack.project, action);
        if (result.failed.length === 0) {
            notify("success", `${DONE[action]} ${stack.project}`);
        } else {
            const first = result.failed[0]!;
            notify("error", `${stack.project}: ${result.failed.length} refused. ${first.name}: ${first.message}`);
        }
    } catch (error) {
        notify("error", `${stack.project}: ${errorText(error)}`);
    } finally {
        docker.refresh();
    }
}
