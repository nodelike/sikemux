import type { DatabaseProfile, Engine, ProfileDraft, ServerTarget, Target, TlsMode } from "./api";

export const POSTGRES_PORT = 5432;
export const MYSQL_PORT = 3306;

export const ENGINES: readonly { value: Engine; label: string }[] = [
    { value: "postgres", label: "PostgreSQL" },
    { value: "mysql", label: "MySQL" },
    { value: "sqlite", label: "SQLite" },
];

export const isServer = (target: Target): target is ServerTarget => target.engine !== "sqlite";

export const defaultPort = (engine: ServerTarget["engine"]): number => (engine === "mysql" ? MYSQL_PORT : POSTGRES_PORT);

export const TLS_MODES: readonly { value: TlsMode; label: string; detail: string }[] = [
    { value: "prefer", label: "Prefer", detail: "Encrypt when the server offers it" },
    { value: "require", label: "Require", detail: "Always encrypt, any certificate" },
    { value: "verify-full", label: "Verify", detail: "Always encrypt, trusted certificate only" },
    { value: "disable", label: "Off", detail: "Never encrypt" },
];

export const engineLabel = (engine: Engine): string => ENGINES.find((entry) => entry.value === engine)?.label ?? engine;

export function blankTarget(engine: Engine): Target {
    return engine === "sqlite" ? { engine, path: "" } : { engine, host: "localhost", port: null, database: "", user: "", tls: "prefer" };
}

export function blankDraft(engine: Engine = "postgres"): ProfileDraft {
    return { name: "", readOnly: false, agentWrites: false, ...blankTarget(engine) };
}

/** The saved profile as the form edits it, without what only the backend decides. */
export function draftOf(profile: DatabaseProfile): ProfileDraft {
    const { hasPassword: _, ...draft } = profile;
    return draft;
}

/** Switching engine keeps the name and the read-only choice, and starts the rest afresh. */
export function withEngine(draft: ProfileDraft, engine: Engine): ProfileDraft {
    if (draft.engine === engine) return draft;
    return { id: draft.id, name: draft.name, readOnly: draft.readOnly, agentWrites: draft.agentWrites, ...blankTarget(engine) };
}

/** What still has to be filled in before the profile can be tried or saved, or null when nothing does. */
export function missingField(draft: ProfileDraft): string | null {
    if (!draft.name.trim()) return "a name";
    if (draft.engine === "sqlite") return draft.path.trim() ? null : "the database file";
    if (!draft.host.trim()) return "a host";
    if (!draft.user.trim()) return "a user name";
    if (draft.port !== null && !(Number.isInteger(draft.port) && draft.port > 0 && draft.port < 65536)) return "a port from 1 to 65535";
    return null;
}

/** A port field's text as the profile keeps it: empty is the default port. */
export function parsePort(text: string): number | null {
    const trimmed = text.trim();
    return trimmed ? Number(trimmed) : null;
}

/** Where the database is, the way the list shows it: `app@localhost:5432/shop`, or the file's name. */
export function addressOf(target: Target): string {
    if (target.engine === "sqlite") return target.path.split("/").pop() || target.path;
    const database = target.database ? `/${target.database}` : "";
    return `${target.user}@${target.host}:${target.port ?? defaultPort(target.engine)}${database}`;
}

/** The schema a database opens on: `public`, `main`, or the database a MySQL profile names. */
export function defaultSchema(target: Target): string {
    if (target.engine === "postgres") return "public";
    if (target.engine === "sqlite") return "main";
    return target.database;
}
