import type { EndedTask, EndedTasks } from "../wire.generated";
import { english, type Say } from "../lib/say";

/** How many tasks the line stands for, a task that ran several times counted each time. */
export const endedCount = (tasks: EndedTasks) => tasks.tasks.reduce((sum, task) => sum + task.times, 0);

/** The line's name, as "7 background tasks". */
export const endedName = (tasks: EndedTasks, say: Say = english) => {
    const count = endedCount(tasks);
    return count === 1 ? say("1 background task") : say("{count} background tasks", { count });
};

/** How many ended each way, as "5 done · 2 watches stopped". */
export function endedHow(tasks: EndedTasks, say: Say = english): string {
    const said: string[] = [];
    if (tasks.done) said.push(say("{count} done", { count: tasks.done }));
    if (tasks.failed) said.push(say("{count} failed", { count: tasks.failed }));
    if (tasks.stopped && tasks.stopped === tasks.watchesStopped) {
        said.push(tasks.stopped === 1 ? say("1 watch stopped") : say("{count} watches stopped", { count: tasks.stopped }));
    } else if (tasks.stopped) {
        said.push(say("{count} stopped", { count: tasks.stopped }));
    }
    return said.join(" · ");
}

/** The state each dot shows, one for each time a task ran. */
export const endedDots = (tasks: EndedTasks, most = 16): EndedTask["state"][] =>
    tasks.tasks.flatMap((task) => Array<EndedTask["state"]>(task.times).fill(task.state)).slice(0, most);

/** A task's name in sentence case. */
export const sentence = (name: string) => name.charAt(0).toUpperCase() + name.slice(1);
