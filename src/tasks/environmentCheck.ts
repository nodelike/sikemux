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
    | { readonly kind: "deps"; readonly ok: boolean; readonly install: string }
    | {
          readonly kind: "port";
          readonly ok: boolean;
          readonly port: number;
          readonly heldBy: string | null;
          /** The next port nothing listens on, to move the app to. */
          readonly freePort: number | null;
          /** The environment variable that moves the app to another port, when there is one. */
          readonly envVar: string | null;
      };

const HOMEBREW_FORMULA: Readonly<Record<string, string>> = { mvn: "maven", npm: "node", python3: "python" };
const NODE_MANAGERS = ["npm", "pnpm", "yarn", "bun"];
const SPRING_PROPERTIES = ["src/main/resources/application.properties", "src/main/resources/application.yml", "src/main/resources/application.yaml"];

interface AppPort {
    readonly port: number;
    readonly envVar: string | null;
}

/** Spring Boot listens on `server.port`, then `SERVER_PORT`, then 8080. */
async function springPort(task: CheckedTask): Promise<AppPort | null> {
    if (!/spring-boot:run|bootRun/.test(task.command)) return null;
    const fromEnv = Number(task.env?.SERVER_PORT);
    if (fromEnv) return { port: fromEnv, envVar: "SERVER_PORT" };
    for (const file of SPRING_PROPERTIES) {
        const text = await fsapi.readTextFileLimited(joinPath(task.cwd, file)).catch(() => "");
        const port = /^\s*server\.port\s*[=:]\s*(\d+)/m.exec(text) ?? /^server:\s*\n(?:\s+.*\n)*?\s+port:\s*(\d+)/m.exec(text);
        if (port) return { port: Number(port[1]), envVar: "SERVER_PORT" };
    }
    return { port: 8080, envVar: "SERVER_PORT" };
}

/**
 * Next.js serves on `-p`, then `PORT`, then 3000. Vite is left out: it moves to
 * the next free port on its own and prints where.
 */
async function nodePort(task: CheckedTask): Promise<AppPort | null> {
    const [manager, verb, script] = task.command.split(" ");
    if (!NODE_MANAGERS.includes(manager) || verb !== "run" || !script) return null;
    const manifest = await fsapi.readTextFileLimited(joinPath(task.cwd, "package.json")).catch(() => "");
    let body: unknown;
    try {
        body = (JSON.parse(manifest) as { scripts?: Record<string, unknown> }).scripts?.[script];
    } catch {
        return null;
    }
    if (typeof body !== "string" || !/\bnext\s+(dev|start)\b/.test(body)) return null;
    const flag = /(?:-p|--port)[\s=](\d+)/.exec(body);
    const port = flag ? Number(flag[1]) : Number(task.env?.PORT) || 3000;
    return { port, envVar: "PORT" };
}

/** uvicorn reads `--port` or `UVICORN_PORT`, Flask `--port` or `FLASK_RUN_PORT`; Django's runserver takes the port as an argument only. */
function pythonPort(task: CheckedTask): AppPort | null {
    const flag = /--port[\s=](\d+)/.exec(task.command);
    if (/-m uvicorn\b/.test(task.command))
        return { port: flag ? Number(flag[1]) : Number(task.env?.UVICORN_PORT) || 8000, envVar: flag ? null : "UVICORN_PORT" };
    if (/-m flask\b.*\brun\b/.test(task.command))
        return { port: flag ? Number(flag[1]) : Number(task.env?.FLASK_RUN_PORT) || 5000, envVar: flag ? null : "FLASK_RUN_PORT" };
    if (/manage\.py runserver\b/.test(task.command)) {
        const address = /runserver\s+(?:[\w.]+:)?(\d+)/.exec(task.command);
        return { port: address ? Number(address[1]) : 8000, envVar: null };
    }
    return null;
}

function holder(port: ListeningPort, ownKeys: readonly string[]): string | null {
    const execution = port.owner.kind === "pty" ? port.owner.taskExecutionId : null;
    const key = execution ? terminalKeyOfExecution(execution) : null;
    if (key && ownKeys.includes(key)) return null;
    return `${port.process || "a process"} (pid ${port.pid})`;
}

async function dependencies(task: CheckedTask): Promise<CheckItem | null> {
    const manager = task.command.split(" ")[0];
    if (!NODE_MANAGERS.includes(manager)) return null;
    const [manifest, modules] = await fsapi
        .pathKinds([joinPath(task.cwd, "package.json"), joinPath(task.cwd, "node_modules")])
        .catch(() => [null, null]);
    if (manifest !== "file") return null;
    return { kind: "deps", ok: modules === "dir", install: `${manager} install` };
}

/** The first port after `port` that neither Sikemux's runs nor any other program listens on. */
async function nextFreePort(port: number, listening: readonly ListeningPort[]): Promise<number | null> {
    const known = new Set(listening.map((candidate) => candidate.port));
    const candidates = Array.from({ length: 50 }, (_, step) => port + step + 1).filter((next) => !known.has(next));
    const busy = new Set(await portsApi.inUse(candidates).catch(() => candidates));
    return candidates.find((next) => !busy.has(next)) ?? null;
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
    const deps = await dependencies(task);
    if (deps) items.push(deps);
    const app = (await springPort(task)) ?? (await nodePort(task)) ?? pythonPort(task);
    if (app) {
        const listening = await portsApi.listening().catch(() => []);
        const taken = listening.find((candidate) => candidate.port === app.port);
        const ours = taken !== undefined && holder(taken, ownKeys) === null;
        const busy = !ours && (taken !== undefined || (await portsApi.inUse([app.port]).catch(() => [])).length > 0);
        const heldBy = busy ? (taken ? holder(taken, ownKeys) : "another program on this Mac") : null;
        items.push({
            kind: "port",
            ok: !busy,
            port: app.port,
            heldBy,
            freePort: busy && app.envVar ? await nextFreePort(app.port, listening) : null,
            envVar: app.envVar,
        });
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
        case "deps":
            return item.ok
                ? { title: "Dependencies installed", detail: "node_modules is there" }
                : { title: "Dependencies aren't installed", detail: `There is no node_modules yet; Install runs ${item.install}` };
        case "port":
            return item.ok
                ? { title: `Port ${item.port} is free`, detail: "Where the app will listen" }
                : {
                      title: `Port ${item.port} is in use`,
                      detail: item.freePort
                          ? `Held by ${item.heldBy}; Fix runs the app on ${item.freePort}`
                          : `Held by ${item.heldBy}; the app can't start until it's free`,
                  };
    }
}

/** One line per problem, for an agent that asked to start the task. */
export function checkProblems(items: readonly CheckItem[]): string | null {
    const problems = items
        .filter((item) => !item.ok)
        .map((item) => {
            const { title, detail } = checkItemText(item);
            if (item.kind === "build-tool")
                return `${title}. ${detail}. Install it with: brew install ${HOMEBREW_FORMULA[item.program] ?? item.program}`;
            if (item.kind === "java") return `${title}. ${detail}. Install it with: brew install openjdk@${item.required}`;
            if (item.kind === "deps") return `${title}. Run \`${item.install}\` in the task's folder first.`;
            return `${title}. ${detail}.`;
        });
    return problems.length ? problems.join("\n") : null;
}
