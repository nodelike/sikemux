import { fsapi } from "../api/fs";
import { joinPath } from "../lib/paths";

export const PROJECT_CONFIG_FILE = "sikemux.json";
export const PROJECT_CONFIG_VERSION = 1 as const;

export type ProjectActionPlacement = "background" | "terminal" | "split" | "popup" | "replace";
export type ProjectCommandContext = "project";

export interface ProjectAction {
    id: string;
    label: string;
    description: string;
    command: string;
    placement: ProjectActionPlacement;
    /** An empty list makes the action available throughout its owning project. */
    contexts: ProjectCommandContext[];
    keybinding?: string;
}

export interface ProjectPreview {
    /** An absolute HTTP(S) URL. Usually points at a local development server. */
    url?: string;
    /** Optional trusted project command that starts the preview server. */
    command?: string;
}

export interface ProjectTask {
    id: string;
    label: string;
    command: string;
    /** Normalized project-relative directory. `.` means the project root. */
    cwd: string;
    env: Record<string, string>;
}

export interface ProjectWorktreeCreateHook {
    id: string;
    label: string;
    command: string;
}

export interface SikemuxProjectConfig {
    version: typeof PROJECT_CONFIG_VERSION;
    /** Optional schema hint for editors. Sikemux does not fetch this URL. */
    $schema?: string;
    /** Project-relative image path. */
    icon?: string;
    actions: ProjectAction[];
    tasks: ProjectTask[];
    preview?: ProjectPreview;
    worktree?: {
        onCreate: ProjectWorktreeCreateHook[];
    };
}

export type ProjectConfigValidationCode =
    | "invalid-json"
    | "invalid-type"
    | "missing-field"
    | "unknown-field"
    | "unsupported-version"
    | "invalid-value"
    | "duplicate-id"
    | "limit-exceeded"
    | "read-failed";

export interface ProjectConfigValidationError {
    /** JSONPath-like location suitable for showing directly in project settings. */
    path: string;
    code: ProjectConfigValidationCode;
    message: string;
}

export interface ProjectConfigTrustSummary {
    requiresApproval: boolean;
    executableEntries: number;
    reasons: string[];
}

export type ProjectConfigLoadResult =
    | { status: "absent"; path: string }
    | {
          status: "invalid";
          path: string;
          errors: ProjectConfigValidationError[];
          /** Present when a file was read, so a changed invalid file can still be detected. */
          fingerprint?: string;
      }
    | {
          status: "valid";
          path: string;
          config: SikemuxProjectConfig;
          fingerprint: string;
          trust: ProjectConfigTrustSummary;
      };

export type ProjectConfigValidationResult = { ok: true; config: SikemuxProjectConfig } | { ok: false; errors: ProjectConfigValidationError[] };

type ReadProjectFile = (path: string) => Promise<string>;
type JsonRecord = Record<string, unknown>;

const ROOT_FIELDS = new Set(["version", "$schema", "icon", "actions", "tasks", "preview", "worktree"]);
const ACTION_FIELDS = new Set(["id", "label", "description", "command", "placement", "contexts", "keybinding"]);
const TASK_FIELDS = new Set(["id", "label", "command", "cwd", "env"]);
const PREVIEW_FIELDS = new Set(["url", "command"]);
const WORKTREE_FIELDS = new Set(["onCreate"]);
const HOOK_FIELDS = new Set(["id", "label", "command"]);
const ACTION_PLACEMENTS = new Set<ProjectActionPlacement>(["background", "terminal", "split", "popup", "replace"]);
const COMMAND_CONTEXTS = new Set<ProjectCommandContext>(["project"]);
const PROJECT_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const MAX_CONFIG_BYTES = 256 * 1024;
const MAX_ACTIONS = 100;
const MAX_TASKS = 100;
const MAX_HOOKS = 20;
const KEYBINDING_MODIFIERS = new Set(["Meta", "Ctrl", "Alt", "Shift"]);
const KEYBINDING_PRIMARY_MODIFIERS = new Set(["Meta", "Ctrl", "Alt"]);
const KEYBINDING_MODIFIER_ORDER = ["Meta", "Ctrl", "Alt", "Shift"] as const;
const PHYSICAL_KEY_CODES = new Set([
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "Backquote",
    "Backslash",
    "Backspace",
    "BracketLeft",
    "BracketRight",
    "Comma",
    "ContextMenu",
    "Delete",
    "End",
    "Enter",
    "Equal",
    "Escape",
    "Help",
    "Home",
    "Insert",
    "IntlBackslash",
    "IntlRo",
    "IntlYen",
    "Minus",
    "PageDown",
    "PageUp",
    "Pause",
    "Period",
    "PrintScreen",
    "Quote",
    "ScrollLock",
    "Semicolon",
    "Slash",
    "Space",
    "Tab",
]);
const MAX_TASK_ENV_ENTRIES = 128;
const MAX_TASK_ENV_VALUE_LENGTH = 8_192;
const MAX_TASK_ENV_TOTAL_LENGTH = 65_536;
const ENVIRONMENT_KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;
const UNSAFE_ENVIRONMENT_KEYS = new Set(["__proto__", "constructor", "prototype"]);

function isRecord(value: unknown): value is JsonRecord {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function issue(path: string, code: ProjectConfigValidationCode, message: string): ProjectConfigValidationError {
    return { path, code, message };
}

function rejectUnknownFields(value: JsonRecord, allowed: ReadonlySet<string>, path: string, errors: ProjectConfigValidationError[]): void {
    for (const key of Object.keys(value)) {
        if (!allowed.has(key)) errors.push(issue(`${path}.${key}`, "unknown-field", `Unknown field “${key}”.`));
    }
}

function boundedRequiredString(
    value: unknown,
    path: string,
    label: string,
    maxLength: number,
    errors: ProjectConfigValidationError[],
): string | null {
    if (typeof value !== "string") {
        errors.push(issue(path, value === undefined ? "missing-field" : "invalid-type", `${label} must be a string.`));
        return null;
    }
    const normalized = value.trim();
    if (!normalized) {
        errors.push(issue(path, "invalid-value", `${label} cannot be empty.`));
        return null;
    }
    if (normalized.length > maxLength) {
        errors.push(issue(path, "limit-exceeded", `${label} must be ${maxLength} characters or fewer.`));
        return null;
    }
    if (normalized.includes("\0")) {
        errors.push(issue(path, "invalid-value", `${label} cannot contain a null byte.`));
        return null;
    }
    return normalized;
}

function boundedOptionalString(
    value: unknown,
    path: string,
    label: string,
    maxLength: number,
    errors: ProjectConfigValidationError[],
): string | undefined {
    if (value === undefined) return undefined;
    return boundedRequiredString(value, path, label, maxLength, errors) ?? undefined;
}

function parseId(value: unknown, path: string, errors: ProjectConfigValidationError[]): string | null {
    const id = boundedRequiredString(value, path, "ID", 64, errors);
    if (id && !PROJECT_ID.test(id)) {
        errors.push(
            issue(path, "invalid-value", "ID must start with a letter or number and contain only letters, numbers, dots, dashes, or underscores."),
        );
        return null;
    }
    return id;
}

function parseCommand(value: unknown, path: string, errors: ProjectConfigValidationError[]): string | null {
    return boundedRequiredString(value, path, "Command", 8_000, errors);
}

function isPhysicalKeyCode(value: string): boolean {
    return (
        /^(?:Key[A-Z]|Digit[0-9]|F(?:[1-9]|1[0-9]|2[0-4]))$/u.test(value) ||
        /^Numpad(?:[0-9]|Add|Comma|Decimal|Divide|Enter|Equal|Multiply|Subtract)$/u.test(value) ||
        PHYSICAL_KEY_CODES.has(value)
    );
}

/** Returns the exact canonical form emitted by `eventToKeybinding`. */
export function normalizeProjectActionKeybinding(value: string): string | null {
    const parts = value.split("+");
    const code = parts.pop();
    if (!code || !isPhysicalKeyCode(code) || parts.length === 0) return null;
    const modifiers = new Set<string>();
    for (const modifier of parts) {
        if (!KEYBINDING_MODIFIERS.has(modifier) || modifiers.has(modifier)) return null;
        modifiers.add(modifier);
    }
    if (![...KEYBINDING_PRIMARY_MODIFIERS].some((modifier) => modifiers.has(modifier))) return null;
    return [...KEYBINDING_MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)), code].join("+");
}

function parseActionKeybinding(value: unknown, path: string, errors: ProjectConfigValidationError[]): string | undefined {
    const binding = boundedOptionalString(value, path, "Keybinding", 100, errors);
    if (binding === undefined) return undefined;
    const normalized = normalizeProjectActionKeybinding(binding);
    if (!normalized) {
        errors.push(
            issue(path, "invalid-value", "Keybinding must contain Meta, Ctrl, or Alt plus one supported physical key code, with optional Shift."),
        );
        return undefined;
    }
    return normalized;
}

function parseActions(value: unknown, errors: ProjectConfigValidationError[]): ProjectAction[] {
    if (value === undefined) return [];
    if (!Array.isArray(value)) {
        errors.push(issue("$.actions", "invalid-type", "Actions must be an array."));
        return [];
    }
    if (value.length > MAX_ACTIONS) errors.push(issue("$.actions", "limit-exceeded", `At most ${MAX_ACTIONS} actions are allowed.`));

    const actions: ProjectAction[] = [];
    const ids = new Set<string>();
    for (const [index, candidate] of value.slice(0, MAX_ACTIONS).entries()) {
        const path = `$.actions[${index}]`;
        if (!isRecord(candidate)) {
            errors.push(issue(path, "invalid-type", "Action must be an object."));
            continue;
        }
        rejectUnknownFields(candidate, ACTION_FIELDS, path, errors);
        const id = parseId(candidate.id, `${path}.id`, errors);
        const label = boundedRequiredString(candidate.label, `${path}.label`, "Label", 120, errors);
        const description = boundedOptionalString(candidate.description, `${path}.description`, "Description", 240, errors) ?? "";
        const command = parseCommand(candidate.command, `${path}.command`, errors);

        let placement: ProjectActionPlacement = "terminal";
        if (candidate.placement !== undefined) {
            if (typeof candidate.placement !== "string" || !ACTION_PLACEMENTS.has(candidate.placement as ProjectActionPlacement)) {
                errors.push(issue(`${path}.placement`, "invalid-value", "Placement must be background, terminal, split, popup, or replace."));
            } else {
                placement = candidate.placement as ProjectActionPlacement;
            }
        }

        const contexts: ProjectCommandContext[] = [];
        if (candidate.contexts !== undefined) {
            if (!Array.isArray(candidate.contexts)) {
                errors.push(issue(`${path}.contexts`, "invalid-type", "Contexts must be an array."));
            } else {
                const contextSet = new Set<ProjectCommandContext>();
                for (const [contextIndex, context] of candidate.contexts.entries()) {
                    if (typeof context !== "string" || !COMMAND_CONTEXTS.has(context as ProjectCommandContext)) {
                        errors.push(issue(`${path}.contexts[${contextIndex}]`, "invalid-value", 'Project action context must be "project".'));
                    } else if (!contextSet.has(context as ProjectCommandContext)) {
                        contextSet.add(context as ProjectCommandContext);
                        contexts.push(context as ProjectCommandContext);
                    }
                }
            }
        }

        const keybinding = parseActionKeybinding(candidate.keybinding, `${path}.keybinding`, errors);
        if (id) {
            if (ids.has(id)) errors.push(issue(`${path}.id`, "duplicate-id", `Action ID “${id}” is duplicated.`));
            ids.add(id);
        }
        if (id && label && command) actions.push({ id, label, description, command, placement, contexts, ...(keybinding ? { keybinding } : {}) });
    }
    return actions;
}

function parseTaskCwd(value: unknown, path: string, errors: ProjectConfigValidationError[]): string {
    const cwd = boundedOptionalString(value, path, "Task working directory", 512, errors) ?? ".";
    const normalized = cwd.replaceAll("\\", "/").replace(/\/+$/, "") || ".";
    const parts = normalized.split("/");
    if (
        containsControlCharacter(normalized) ||
        normalized.startsWith("/") ||
        /^[A-Za-z]:\//.test(normalized) ||
        normalized.startsWith("~/") ||
        parts.some((part) => part === "" || part === ".." || (part === "." && normalized !== "."))
    ) {
        errors.push(issue(path, "invalid-value", "Task working directory must stay within the project and contain no dot segments."));
        return ".";
    }
    return normalized;
}

function parseTaskEnv(value: unknown, path: string, errors: ProjectConfigValidationError[]): Record<string, string> {
    if (value === undefined) return {};
    if (!isRecord(value)) {
        errors.push(issue(path, "invalid-type", "Task environment must be an object of string values."));
        return {};
    }
    const entries = Object.entries(value);
    if (entries.length > MAX_TASK_ENV_ENTRIES) {
        errors.push(issue(path, "limit-exceeded", `Task environment may contain at most ${MAX_TASK_ENV_ENTRIES} entries.`));
    }
    let totalLength = 0;
    const env: Record<string, string> = {};
    for (const [key, raw] of entries.slice(0, MAX_TASK_ENV_ENTRIES)) {
        const entryPath = `${path}.${key}`;
        if (!ENVIRONMENT_KEY.test(key) || UNSAFE_ENVIRONMENT_KEYS.has(key)) {
            errors.push(issue(entryPath, "invalid-value", "Environment variable names must use letters, numbers, and underscores."));
            continue;
        }
        if (typeof raw !== "string") {
            errors.push(issue(entryPath, "invalid-type", "Environment variable values must be strings."));
            continue;
        }
        if (raw.includes("\0")) {
            errors.push(issue(entryPath, "invalid-value", "Environment variable values cannot contain a null byte."));
            continue;
        }
        if (raw.length > MAX_TASK_ENV_VALUE_LENGTH) {
            errors.push(issue(entryPath, "limit-exceeded", `Environment variable values must be ${MAX_TASK_ENV_VALUE_LENGTH} characters or fewer.`));
            continue;
        }
        totalLength += key.length + raw.length;
        if (totalLength > MAX_TASK_ENV_TOTAL_LENGTH) {
            errors.push(issue(path, "limit-exceeded", `Task environment must be ${MAX_TASK_ENV_TOTAL_LENGTH} characters or fewer in total.`));
            break;
        }
        env[key] = raw;
    }
    return env;
}

function parseTasks(value: unknown, errors: ProjectConfigValidationError[]): ProjectTask[] {
    if (value === undefined) return [];
    if (!Array.isArray(value)) {
        errors.push(issue("$.tasks", "invalid-type", "Tasks must be an array."));
        return [];
    }
    if (value.length > MAX_TASKS) errors.push(issue("$.tasks", "limit-exceeded", `At most ${MAX_TASKS} tasks are allowed.`));

    const tasks: ProjectTask[] = [];
    const ids = new Set<string>();
    for (const [index, candidate] of value.slice(0, MAX_TASKS).entries()) {
        const path = `$.tasks[${index}]`;
        if (!isRecord(candidate)) {
            errors.push(issue(path, "invalid-type", "Task must be an object."));
            continue;
        }
        rejectUnknownFields(candidate, TASK_FIELDS, path, errors);
        const id = parseId(candidate.id, `${path}.id`, errors);
        const label = boundedRequiredString(candidate.label, `${path}.label`, "Label", 120, errors);
        const command = parseCommand(candidate.command, `${path}.command`, errors);
        const cwd = parseTaskCwd(candidate.cwd, `${path}.cwd`, errors);
        const env = parseTaskEnv(candidate.env, `${path}.env`, errors);
        if (id) {
            if (ids.has(id)) errors.push(issue(`${path}.id`, "duplicate-id", `Task ID “${id}” is duplicated.`));
            ids.add(id);
        }
        if (id && label && command) tasks.push({ id, label, command, cwd, env });
    }
    return tasks;
}

function parsePreview(value: unknown, errors: ProjectConfigValidationError[]): ProjectPreview | undefined {
    if (value === undefined) return undefined;
    if (!isRecord(value)) {
        errors.push(issue("$.preview", "invalid-type", "Preview must be an object."));
        return undefined;
    }
    rejectUnknownFields(value, PREVIEW_FIELDS, "$.preview", errors);
    const url = boundedOptionalString(value.url, "$.preview.url", "Preview URL", 2_048, errors);
    const command = value.command === undefined ? undefined : (parseCommand(value.command, "$.preview.command", errors) ?? undefined);
    if (!url && !command) errors.push(issue("$.preview", "missing-field", "Preview must define at least a URL or command."));
    if (url) {
        try {
            const parsed = new URL(url);
            if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
                errors.push(issue("$.preview.url", "invalid-value", "Preview URL must use HTTP or HTTPS."));
            }
            if (parsed.username || parsed.password) {
                errors.push(issue("$.preview.url", "invalid-value", "Preview URL cannot contain embedded credentials."));
            }
        } catch {
            errors.push(issue("$.preview.url", "invalid-value", "Preview URL must be an absolute URL."));
        }
    }
    return url || command ? { ...(url ? { url } : {}), ...(command ? { command } : {}) } : undefined;
}

function parseWorktree(value: unknown, errors: ProjectConfigValidationError[]): SikemuxProjectConfig["worktree"] {
    if (value === undefined) return undefined;
    if (!isRecord(value)) {
        errors.push(issue("$.worktree", "invalid-type", "Worktree configuration must be an object."));
        return undefined;
    }
    rejectUnknownFields(value, WORKTREE_FIELDS, "$.worktree", errors);
    if (!Array.isArray(value.onCreate)) {
        errors.push(issue("$.worktree.onCreate", value.onCreate === undefined ? "missing-field" : "invalid-type", "onCreate must be an array."));
        return undefined;
    }
    if (value.onCreate.length > MAX_HOOKS) {
        errors.push(issue("$.worktree.onCreate", "limit-exceeded", `At most ${MAX_HOOKS} worktree hooks are allowed.`));
    }

    const hooks: ProjectWorktreeCreateHook[] = [];
    const ids = new Set<string>();
    for (const [index, candidate] of value.onCreate.slice(0, MAX_HOOKS).entries()) {
        const path = `$.worktree.onCreate[${index}]`;
        if (!isRecord(candidate)) {
            errors.push(issue(path, "invalid-type", "Worktree hook must be an object."));
            continue;
        }
        rejectUnknownFields(candidate, HOOK_FIELDS, path, errors);
        const id = parseId(candidate.id, `${path}.id`, errors);
        const label = boundedOptionalString(candidate.label, `${path}.label`, "Label", 120, errors) ?? id ?? "";
        const command = parseCommand(candidate.command, `${path}.command`, errors);
        if (id) {
            if (ids.has(id)) errors.push(issue(`${path}.id`, "duplicate-id", `Worktree hook ID “${id}” is duplicated.`));
            ids.add(id);
        }
        if (id && label && command) hooks.push({ id, label, command });
    }
    return { onCreate: hooks };
}

function parseIcon(value: unknown, errors: ProjectConfigValidationError[]): string | undefined {
    const icon = boundedOptionalString(value, "$.icon", "Icon path", 512, errors);
    if (!icon) return undefined;
    const normalized = icon.replaceAll("\\", "/");
    const parts = normalized.split("/");
    if (normalized.startsWith("/") || /^[A-Za-z]:\//.test(normalized) || normalized.startsWith("~/") || parts.includes("..") || parts.includes(".")) {
        errors.push(issue("$.icon", "invalid-value", "Icon must be a normalized project-relative path without dot segments."));
        return undefined;
    }
    return normalized;
}

export function validateProjectConfig(value: unknown): ProjectConfigValidationResult {
    if (!isRecord(value)) return { ok: false, errors: [issue("$", "invalid-type", "Project configuration must be a JSON object.")] };

    const errors: ProjectConfigValidationError[] = [];
    rejectUnknownFields(value, ROOT_FIELDS, "$", errors);
    if (value.version !== PROJECT_CONFIG_VERSION) {
        errors.push(
            issue(
                "$.version",
                value.version === undefined ? "missing-field" : "unsupported-version",
                value.version === undefined
                    ? `Version is required. The current version is ${PROJECT_CONFIG_VERSION}.`
                    : `Unsupported project configuration version “${String(value.version)}”; expected ${PROJECT_CONFIG_VERSION}.`,
            ),
        );
    }
    const schema = boundedOptionalString(value.$schema, "$.$schema", "Schema URL", 2_048, errors);
    const icon = parseIcon(value.icon, errors);
    const actions = parseActions(value.actions, errors);
    const tasks = parseTasks(value.tasks, errors);
    const preview = parsePreview(value.preview, errors);
    const worktree = parseWorktree(value.worktree, errors);

    if (errors.length) return { ok: false, errors };
    return {
        ok: true,
        config: {
            version: PROJECT_CONFIG_VERSION,
            ...(schema ? { $schema: schema } : {}),
            ...(icon ? { icon } : {}),
            actions,
            tasks,
            ...(preview ? { preview } : {}),
            ...(worktree ? { worktree } : {}),
        },
    };
}

export function projectConfigTrustSummary(config: SikemuxProjectConfig): ProjectConfigTrustSummary {
    const actionCommands = config.actions.length;
    const taskCommands = config.tasks.length;
    const previewCommands = config.preview?.command ? 1 : 0;
    const worktreeCommands = config.worktree?.onCreate.length ?? 0;
    const reasons: string[] = [];
    if (actionCommands) reasons.push(`${actionCommands} project ${actionCommands === 1 ? "action" : "actions"}`);
    if (taskCommands) reasons.push(`${taskCommands} project ${taskCommands === 1 ? "task" : "tasks"}`);
    if (previewCommands) reasons.push("a preview command");
    if (worktreeCommands) reasons.push(`${worktreeCommands} worktree-create ${worktreeCommands === 1 ? "hook" : "hooks"}`);
    const executableEntries = actionCommands + taskCommands + previewCommands + worktreeCommands;
    return { requiresApproval: executableEntries > 0, executableEntries, reasons };
}

/** A content-addressed token suitable for invalidating an earlier trust decision. */
export async function fingerprintProjectConfigSource(source: string): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(source));
    const hex = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
    return `sha256:${hex}`;
}

function isMissingFileError(error: unknown): boolean {
    const detail = error instanceof Error ? `${error.name} ${error.message}` : String(error);
    return /\b(enoent|not found|no such file|os error 2)\b/i.test(detail);
}

export async function loadProjectConfig(rootPath: string, readFile: ReadProjectFile = fsapi.readFile): Promise<ProjectConfigLoadResult> {
    const path = joinPath(rootPath, PROJECT_CONFIG_FILE);
    let source: string;
    try {
        source = await readFile(path);
    } catch (error) {
        if (isMissingFileError(error)) return { status: "absent", path };
        const detail = error instanceof Error ? error.message : String(error);
        return { status: "invalid", path, errors: [issue("$", "read-failed", `Could not read ${PROJECT_CONFIG_FILE}: ${detail}`)] };
    }

    if (new TextEncoder().encode(source).byteLength > MAX_CONFIG_BYTES) {
        return {
            status: "invalid",
            path,
            fingerprint: await fingerprintProjectConfigSource(source),
            errors: [issue("$", "limit-exceeded", `Project configuration must be ${MAX_CONFIG_BYTES / 1024} KiB or smaller.`)],
        };
    }

    const fingerprint = await fingerprintProjectConfigSource(source);
    let value: unknown;
    try {
        value = JSON.parse(source) as unknown;
    } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        return { status: "invalid", path, fingerprint, errors: [issue("$", "invalid-json", `Invalid JSON: ${detail}`)] };
    }

    const result = validateProjectConfig(value);
    if (!result.ok) return { status: "invalid", path, fingerprint, errors: result.errors };
    return { status: "valid", path, fingerprint, config: result.config, trust: projectConfigTrustSummary(result.config) };
}
