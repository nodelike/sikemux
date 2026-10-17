import { buildTools } from "../api/buildTools";
import { fsapi } from "../api/fs";
import { portsApi, type ListeningPort } from "../api/ports";
import { joinPath } from "../lib/paths";
import { buildToolOf } from "./detectedTasks";
import type { JavaChoice } from "./javaToolchain";
import { terminalKeyOfExecution } from "./runtime";

interface CheckedTask {
    readonly id: string;
    readonly label: string;
    readonly command: string;
    readonly cwd: string;
    readonly env?: Readonly<Record<string, string>>;
}

export type CheckItem =
    | { readonly kind: "build-tool"; readonly ok: boolean; readonly name: string; readonly program: string }
    | {
          readonly kind: "java";
          readonly ok: boolean;
          readonly required: number;
          readonly using: number | null;
          readonly current: number | null;
      }
    | { readonly kind: "port"; readonly ok: boolean; readonly port: number; readonly heldBy: string | null };

const SPRING_PROPERTIES = ["src/main/resources/application.properties", "src/main/resources/application.yml", "src/main/resources/application.yaml"];

/** Spring Boot listens on `server.port`, then `SERVER_PORT`, then 8080. */
async function springPort(task: CheckedTask): Promise<number | null> {
    if (!/spring-boot:run|bootRun/.test(task.command)) return null;
    const fromEnv = Number(task.env?.SERVER_PORT);
    if (fromEnv) return fromEnv;
    for (const file of SPRING_PROPERTIES) {
        const text = await fsapi.readTextFileLimited(joinPath(task.cwd, file)).catch(() => "");
        const port = /^\s*server\.port\s*[=:]\s*(\d+)/m.exec(text) ?? /^server:\s*\n(?:\s+.*\n)*?\s+port:\s*(\d+)/m.exec(text);
        if (port) return Number(port[1]);
    }
    return 8080;
}

function holder(port: ListeningPort, ownKeys: readonly string[]): string | null {
    const execution = port.owner.kind === "pty" ? port.owner.taskExecutionId : null;
    const key = execution ? terminalKeyOfExecution(execution) : null;
    if (key && ownKeys.includes(key)) return null;
    return `${port.process || "a process"} (pid ${port.pid})`;
}

/**
 * What would stop a task from starting, found before it runs. `ownKeys` are the
 * terminal keys of this task's own earlier runs, whose port a restart frees.
 */
export async function checkTask(task: CheckedTask, java: JavaChoice | null, ownKeys: readonly string[] = []): Promise<CheckItem[]> {
    const items: CheckItem[] = [];
    const tool = buildToolOf(task.command);
    if (tool) items.push({ kind: "build-tool", ok: await buildTools.installed(tool.program), ...tool });
    if (java)
        items.push({
            kind: "java",
            ok: java.runtime !== null,
            required: java.required,
            using: java.runtime?.major ?? null,
            current: java.current?.major ?? null,
        });
    const port = await springPort(task);
    if (port) {
        const listening = await portsApi.listening().catch(() => []);
        const taken = listening.find((candidate) => candidate.port === port);
        const heldBy = taken ? holder(taken, ownKeys) : null;
        items.push({ kind: "port", ok: heldBy === null, port, heldBy });
    }
    return items;
}

export function checkItemText(item: CheckItem): { title: string; detail: string } {
    switch (item.kind) {
        case "build-tool":
            return item.ok
                ? { title: item.name, detail: `${item.program} is installed` }
                : { title: `${item.name} isn't installed`, detail: `This task runs ${item.program}` };
        case "java":
            if (item.ok)
                return {
                    title: `Java ${item.using}`,
                    detail:
                        item.using === item.current
                            ? `The project asks for Java ${item.required}`
                            : `The project asks for Java ${item.required}; your JAVA_HOME is ${item.current ?? "not set"}`,
                };
            return {
                title: `Java ${item.required} needed`,
                detail: `The project asks for Java ${item.required}; ${item.current ? `your Java is ${item.current}` : "no Java is set"} and nothing newer is installed`,
            };
        case "port":
            return item.ok
                ? { title: `Port ${item.port} is free`, detail: "Where the app will listen" }
                : { title: `Port ${item.port} is in use`, detail: `Held by ${item.heldBy}; the app can't start until it's free` };
    }
}

/** One line per problem, for an agent that asked to start the task. */
export function checkProblems(items: readonly CheckItem[]): string | null {
    const problems = items
        .filter((item) => !item.ok)
        .map((item) => {
            const { title, detail } = checkItemText(item);
            if (item.kind === "build-tool")
                return `${title}. ${detail}. Install it with: brew install ${item.program === "mvn" ? "maven" : item.program}`;
            if (item.kind === "java") return `${title}. ${detail}. Install it with: brew install openjdk@${item.required}`;
            return `${title}. ${detail}.`;
        });
    return problems.length ? problems.join("\n") : null;
}
