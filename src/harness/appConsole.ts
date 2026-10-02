import { swallowedErrors } from "../state/toast";

type Level = "log" | "info" | "warn" | "error" | "debug" | "uncaught" | "unhandled rejection" | "swallowed";

interface Entry {
    level: Level;
    at: number;
    text: string;
}

const MAX_ENTRIES = 500;
const MAX_TEXT = 2000;
const CONSOLE_LEVELS = ["log", "info", "warn", "error", "debug"] as const;
const QUIET_LEVELS: Level[] = ["log", "info", "debug"];
const TOO_LARGE = Symbol("too large");

function clip(text: string): string {
    return text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT)}…` : text;
}

function describeObject(value: object): string {
    let budget = MAX_TEXT;
    try {
        const json = JSON.stringify(value, (key, part: unknown) => {
            budget -= key.length + (typeof part === "string" ? part.length : 4);
            if (budget < 0) throw TOO_LARGE;
            return part;
        });
        return json ?? String(value);
    } catch (error) {
        return error === TOO_LARGE ? `${Object.prototype.toString.call(value)} (too large to show)` : String(value);
    }
}

function describe(value: unknown): string {
    if (typeof value === "string") return value;
    if (value instanceof Error) return [`${value.name}: ${value.message}`, value.stack].filter(Boolean).join("\n");
    if (typeof Node === "function" && value instanceof Node) return `<${value.nodeName.toLowerCase()}>`;
    if (typeof value === "object" && value !== null) return describeObject(value);
    return String(value);
}

/** The main window's own console, kept so an agent can read it without Web Inspector. */
export class AppConsole {
    private readonly entries: Entry[] = [];
    private installed = false;

    constructor(private readonly swallowed: () => readonly { ts: number; label: string; err: unknown }[] = swallowedErrors) {}

    add(level: Level, parts: readonly unknown[]): void {
        this.entries.push({ level, at: Date.now(), text: clip(parts.map(describe).join(" ")) });
        if (this.entries.length > MAX_ENTRIES) this.entries.shift();
    }

    install(target: Window & typeof globalThis): void {
        if (this.installed) return;
        this.installed = true;
        for (const level of CONSOLE_LEVELS) {
            const original = target.console[level];
            if (typeof original !== "function") continue;
            target.console[level] = (...parts: unknown[]) => {
                this.add(level, parts);
                original.apply(target.console, parts);
            };
        }
        target.addEventListener("error", (event) => {
            if (!event.error && !event.message) return;
            const problem: unknown = event.error ?? event.message;
            this.add("uncaught", event.filename ? [problem, `(${event.filename}:${event.lineno}:${event.colno})`] : [problem]);
        });
        target.addEventListener("unhandledrejection", (event) => this.add("unhandled rejection", [event.reason]));
    }

    read(params: Record<string, unknown>) {
        const limit = params.limit ?? 50;
        if (typeof limit !== "number" || !Number.isSafeInteger(limit) || limit < 1 || limit > 200)
            throw new Error("limit must be an integer between 1 and 200");
        if (params.errors !== undefined && typeof params.errors !== "boolean") throw new Error("errors must be a boolean");
        const swallowed: Entry[] = this.swallowed().map(({ ts, label, err }) => ({
            level: "swallowed",
            at: ts,
            text: clip(`${label}: ${describe(err)}`),
        }));
        const all = [...this.entries, ...swallowed].sort((left, right) => left.at - right.at);
        const matched = params.errors ? all.filter((entry) => !QUIET_LEVELS.includes(entry.level)) : all;
        return {
            recorded: all.length,
            matched: matched.length,
            messages: matched.slice(-limit).map((entry) => ({ ...entry, at: new Date(entry.at).toISOString() })),
        };
    }
}

export const appConsole = new AppConsole();
