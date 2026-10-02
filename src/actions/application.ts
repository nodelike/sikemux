import { ActionRegistry, type ActionContextInput, type ActionDefinition, type ActionScope } from "./registry";
import { keybindingLabel, matchesKeybinding } from "../commands/keybindings";
import { normalizeProjectActionKeybinding, type ProjectAction } from "../projects/projectConfig";
import {
    InternalExtensionHost,
    type ExtensionContributionAdapter,
    type InternalExtensionHostSnapshot,
    type InternalExtensionManifest,
    type InternalExtensionRegistration,
} from "../extensions/host";

export interface ApplicationActionContribution {
    /** Public command ID retained by the command palette and recents. */
    readonly commandId: string;
    readonly definition: ActionDefinition;
    readonly scope?: ActionScope;
}

export type ApplicationActionExtensionManifest = InternalExtensionManifest<ApplicationActionContribution, never, never>;

export interface ApplicationResolvedAction {
    readonly actionId: string;
    readonly commandId: string;
    readonly title: string;
    readonly detail: string;
    readonly category: string;
    readonly source: string;
    readonly binding: string | null;
    readonly shortcut: string;
    readonly enabled: boolean;
}

export interface ApplicationActionMatch {
    readonly actionId: string;
    readonly commandId: string;
}

export interface ProjectActionExtensionOptions {
    readonly projectId: string;
    readonly projectRoot: string;
    readonly configPath: string;
    readonly actions: readonly ProjectAction[];
    /** Revalidated during resolution and immediately before command execution. */
    readonly isCurrent: () => boolean;
    readonly execute: (action: ProjectAction) => unknown | PromiseLike<unknown>;
}

export class StaleProjectActionConfigurationError extends Error {
    constructor() {
        super("Project action configuration is no longer current");
        this.name = "StaleProjectActionConfigurationError";
    }
}

interface ContributionMetadata {
    readonly commandId: string;
    readonly actionId: string;
}

const MAX_SUBSCRIBERS = 128;
const MAX_COMMAND_ID_LENGTH = 256;
const COMMAND_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]*$/u;
const UNSAFE_SEGMENTS = new Set(["__proto__", "constructor", "prototype"]);

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function requireCommandId(value: unknown): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > MAX_COMMAND_ID_LENGTH ||
        value !== value.trim() ||
        containsControlCharacter(value) ||
        !COMMAND_ID_PATTERN.test(value)
    ) {
        throw new TypeError("application command IDs must be bounded record-safe identifiers");
    }
    return value;
}

function safeActionText(value: string, fallback: string, maxLength: number): string {
    let sanitized = "";
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        sanitized += code <= 31 || (code >= 127 && code <= 159) ? " " : value[index];
    }
    const normalized = sanitized.trim() || fallback;
    const bounded = normalized.slice(0, maxLength);
    return UNSAFE_SEGMENTS.has(bounded) ? `${bounded} action` : bounded;
}

function stableIdentityHash(value: string): string {
    let first = 2_166_136_261;
    let second = 2_246_822_519;
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        first = Math.imul(first ^ code, 16_777_619);
        second = Math.imul(second ^ code, 3_266_489_917);
    }
    return `${(first >>> 0).toString(36)}${(second >>> 0).toString(36)}`;
}

function projectExtensionId(projectId: string, projectRoot: string): string {
    return `sikemux.project-actions.p${stableIdentityHash(`${projectId}\u0000${projectRoot}`)}`;
}

function projectActionId(actionId: string): string {
    return `projectConfig.action.p${stableIdentityHash(actionId)}`;
}

function unsupportedAdapter(kind: string): ExtensionContributionAdapter<never> {
    return {
        register(): never {
            throw new TypeError(`${kind} contributions are not supported by the action host`);
        },
    };
}

function copyProjectAction(action: ProjectAction): ProjectAction {
    return Object.freeze({
        ...action,
        contexts: action.contexts.slice(),
    });
}

/**
 * Production adapter between the renderer's static extension host and its
 * contextual action registry. The module is loaded lazily through `bridge.ts`.
 */
export class ApplicationActionRuntime {
    private readonly registry = new ActionRegistry();
    private readonly metadata = new Map<string, ContributionMetadata>();
    private readonly commandOwners = new Map<string, { readonly actionId: string; count: number }>();
    private readonly listeners = new Set<() => void>();
    private readonly host: InternalExtensionHost<ApplicationActionContribution, never, never>;
    private disposed = false;
    private mutationDepth = 0;
    private publishPending = false;

    constructor() {
        const actionAdapter: ExtensionContributionAdapter<ApplicationActionContribution> = {
            register: (value) => this.registerContribution(value),
        };
        this.host = new InternalExtensionHost({
            actions: actionAdapter,
            workbenchItems: unsupportedAdapter("workbench item"),
            taskProviders: unsupportedAdapter("task provider"),
        });
    }

    subscribe(listener: () => void): () => void {
        if (typeof listener !== "function") throw new TypeError("application action subscriber must be a function");
        if (this.disposed) return () => undefined;
        if (this.listeners.size >= MAX_SUBSCRIBERS) {
            throw new RangeError(`application actions cannot exceed ${MAX_SUBSCRIBERS} subscribers`);
        }
        this.listeners.add(listener);
        return () => this.listeners.delete(listener);
    }

    register(manifest: ApplicationActionExtensionManifest): InternalExtensionRegistration {
        return this.batch(() => {
            const registration = this.host.register(manifest);
            return Object.freeze({
                id: registration.id,
                get disposed() {
                    return registration.disposed;
                },
                get activeContributions() {
                    return registration.activeContributions;
                },
                dispose: () => this.batch(() => registration.dispose()),
            });
        });
    }

    registerProjectActions(options: ProjectActionExtensionOptions): InternalExtensionRegistration {
        if (typeof options.isCurrent !== "function") throw new TypeError("project action current-config guard must be a function");
        const isCurrent = (): boolean => {
            try {
                return options.isCurrent() === true;
            } catch {
                return false;
            }
        };
        const actions = options.actions
            .filter((action) => action.contexts.length === 0 || action.contexts.includes("project"))
            .map(copyProjectAction);
        return this.register({
            id: projectExtensionId(options.projectId, options.projectRoot),
            actions: actions.map((action, index) => ({
                id: `action-${index}`,
                create: (): ApplicationActionContribution => ({
                    commandId: `project.action.${action.id}`,
                    scope: { kind: "project", id: options.projectId },
                    definition: {
                        id: projectActionId(action.id),
                        title: safeActionText(action.label, "Project action", 256),
                        detail: safeActionText(action.description || `Run from ${options.configPath}`, "Run project action", 2_048),
                        category: "Project · sikemux.json",
                        source: "project.config",
                        defaultBinding: action.keybinding ? normalizeProjectActionKeybinding(action.keybinding) : null,
                        when: (context) => context.project?.root === options.projectRoot && isCurrent(),
                        run: () => {
                            if (!isCurrent()) throw new StaleProjectActionConfigurationError();
                            return options.execute(action);
                        },
                    },
                }),
            })),
        });
    }

    resolve(context: ActionContextInput): readonly ApplicationResolvedAction[] {
        const resolved = this.registry.resolve(context);
        return Object.freeze(
            resolved.flatMap((action): ApplicationResolvedAction[] => {
                const metadata = this.metadata.get(action.contributionId);
                if (!metadata) return [];
                const binding = action.definition.defaultBinding;
                return [
                    Object.freeze({
                        actionId: metadata.actionId,
                        commandId: metadata.commandId,
                        title: action.definition.title,
                        detail: action.definition.detail,
                        category: action.definition.category,
                        source: action.definition.source,
                        binding,
                        shortcut: binding ? keybindingLabel(binding) : "",
                        enabled: action.enabled,
                    }),
                ];
            }),
        );
    }

    matchKeybinding(
        event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
        context: ActionContextInput,
    ): ApplicationActionMatch | null {
        for (const action of this.resolve(context)) {
            if (!action.enabled || !action.binding || !matchesKeybinding(event, action.binding)) continue;
            return Object.freeze({ actionId: action.actionId, commandId: action.commandId });
        }
        return null;
    }

    execute(actionId: string, context: ActionContextInput): Promise<unknown> {
        return this.registry.execute(actionId, context);
    }

    getHostSnapshot(): InternalExtensionHostSnapshot {
        return this.host.getSnapshot();
    }

    dispose(): void {
        if (this.disposed) return;
        this.disposed = true;
        this.batch(() => this.host.dispose());
        this.registry.dispose();
        this.listeners.clear();
    }

    private registerContribution(value: ApplicationActionContribution): { dispose(): void } {
        if (typeof value !== "object" || value === null) throw new TypeError("application action contributions must be objects");
        const commandId = requireCommandId(value.commandId);
        const actionId = value.definition?.id;
        const owner = this.commandOwners.get(commandId);
        if (owner && owner.actionId !== actionId) throw new TypeError(`application command ID is already owned: ${commandId}`);

        const registration = this.registry.register(value.definition, { scope: value.scope });
        const metadata = Object.freeze({ commandId, actionId: registration.definition.id });
        this.metadata.set(registration.contributionId, metadata);
        if (owner) owner.count += 1;
        else this.commandOwners.set(commandId, { actionId: metadata.actionId, count: 1 });
        this.publish();

        let active = true;
        return {
            dispose: () => {
                if (!active) return;
                active = false;
                registration.dispose();
                this.metadata.delete(registration.contributionId);
                const currentOwner = this.commandOwners.get(commandId);
                if (currentOwner) {
                    currentOwner.count -= 1;
                    if (currentOwner.count === 0) this.commandOwners.delete(commandId);
                }
                this.publish();
            },
        };
    }

    private publish(): void {
        if (this.mutationDepth > 0) {
            this.publishPending = true;
            return;
        }
        this.notifyListeners();
    }

    private batch<Result>(operation: () => Result): Result {
        this.mutationDepth += 1;
        try {
            return operation();
        } finally {
            this.mutationDepth -= 1;
            if (this.mutationDepth === 0 && this.publishPending) {
                this.publishPending = false;
                this.notifyListeners();
            }
        }
    }

    private notifyListeners(): void {
        for (const listener of Array.from(this.listeners)) {
            try {
                listener();
            } catch {
                this.listeners.delete(listener);
            }
        }
    }
}

export const applicationActionRuntime = new ApplicationActionRuntime();
