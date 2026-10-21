import { errorText } from "../../plugin-api/format";
import { docker } from "./resources";
import { confirmDialog, copyWithNotice, notify, runInTerminal, swallow } from "../../plugin-api/host";
import type { ContextMenuItem } from "../../plugin-api/ui";
import { dockerApi, type Container, type ContainerAction } from "./api";
import { isRunning } from "./format";

const DONE: Record<ContainerAction, string> = {
    start: "Started",
    stop: "Stopped",
    restart: "Restarted",
    pause: "Paused",
    unpause: "Resumed",
    kill: "Killed",
};

const DOING: Record<ContainerAction, string> = {
    start: "Start",
    stop: "Stop",
    restart: "Restart",
    pause: "Pause",
    unpause: "Resume",
    kill: "Kill",
};

export async function act(name: string, action: ContainerAction): Promise<boolean> {
    if (action === "kill") {
        const sure = await confirmDialog({
            title: `Kill ${name}?`,
            body: "It stops at once, without the chance to finish what it is doing.",
            confirmLabel: "Kill",
            destructive: true,
        });
        if (!sure) return false;
    }
    try {
        await dockerApi.act(name, action);
        notify("success", `${DONE[action]} ${name}`);
        return true;
    } catch (error) {
        notify("error", `${DOING[action]} ${name}: ${errorText(error)}`);
        return false;
    } finally {
        docker.refresh();
    }
}

/** Asks first, and stops a running container on the way, since the engine won't remove one that runs. */
export async function remove(name: string, running: boolean): Promise<boolean> {
    const sure = await confirmDialog({
        title: running ? `Stop and remove ${name}?` : `Remove ${name}?`,
        body: "Its volumes stay. Its own changes to the files inside it are lost.",
        confirmLabel: "Remove",
        destructive: true,
    });
    if (!sure) return false;
    try {
        await dockerApi.remove(name, running);
        notify("success", `Removed ${name}`);
        return true;
    } catch (error) {
        notify("error", `Remove ${name}: ${errorText(error)}`);
        return false;
    } finally {
        docker.refresh();
    }
}

/** A shell inside the container, in a terminal, with bash where the image has it. */
export function shellArgv(name: string): string[] {
    return ["docker", "exec", "-it", name, "sh", "-c", "command -v bash >/dev/null && exec bash || exec sh"];
}

export function openShell(name: string): void {
    void runInTerminal(`Shell in ${name}`, shellArgv(name)).catch(swallow("open a shell in the container"));
}

/** What a container's row offers when right-clicked, most likely first. */
export function containerMenu(container: Container, open: (tab: "logs" | "inspect") => void): ContextMenuItem[] {
    const running = isRunning(container);
    const paused = container.state === "paused";
    return [
        { label: "Logs", run: () => open("logs") },
        { label: "Inspect", run: () => open("inspect") },
        { sep: true },
        running ? { label: "Stop", run: () => void act(container.name, "stop") } : { label: "Start", run: () => void act(container.name, "start") },
        { label: "Restart", disabled: !running, run: () => void act(container.name, "restart") },
        paused
            ? { label: "Resume", run: () => void act(container.name, "unpause") }
            : { label: "Pause", disabled: !running, run: () => void act(container.name, "pause") },
        { label: "Open shell", disabled: !running || paused, run: () => openShell(container.name) },
        { sep: true },
        { label: "Copy name", run: () => copyWithNotice(container.name, "Container name") },
        { label: "Copy ID", run: () => copyWithNotice(container.id, "Container ID") },
        { sep: true },
        { label: "Remove…", danger: true, run: () => void remove(container.name, running) },
    ];
}
