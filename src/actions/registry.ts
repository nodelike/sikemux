declare const ACTION_CONTRIBUTION_ID_BRAND: unique symbol;

export type ActionContributionId = string & {
    readonly [ACTION_CONTRIBUTION_ID_BRAND]: "ActionContributionId";
};

export type ActionScopeKind = "global" | "project" | "session" | "focused-item";

export type ActionScope =
    | { readonly kind: "global" }
    | { readonly kind: "project"; readonly id: string }
    | { readonly kind: "session"; readonly id: string }
    | { readonly kind: "focused-item"; readonly id: string };

export interface ActionItemContext {
    readonly id: string;
    readonly kind: string;
}

export interface ActionSessionContext {
    readonly id: string;
    readonly kind: string;
}

export interface ActionProjectContext {
    readonly id: string;
    readonly root: string;
}

export interface ActionFocusContext {
    readonly target: string;
    readonly editable: boolean;
}

export interface ActionModalContext {
    readonly id: string;
    readonly kind: string;
}

export interface ActionAgentContext {
    readonly id: string;
    readonly kind: string;
    readonly status: string;
}

export interface ActionContext {
    readonly focusedItem: ActionItemContext | null;
    readonly session: ActionSessionContext | null;
    readonly project: ActionProjectContext | null;
    readonly focus: ActionFocusContext;
    readonly modal: ActionModalContext | null;
    readonly agent: ActionAgentContext | null;
    readonly capabilities: readonly string[];
}

export interface ActionContextInput {
    readonly focusedItem?: ActionItemContext | null;
    readonly session?: ActionSessionContext | null;
    readonly project?: ActionProjectContext | null;
    readonly focus?: ActionFocusContext;
    readonly modal?: ActionModalContext | null;
    readonly agent?: ActionAgentContext | null;
    readonly capabilities?: Iterable<string>;
}

export type ActionPredicate = (context: ActionContext) => boolean;

export interface ActionDefinition<Result = unknown> {
    readonly id: string;
    readonly title: string;
    readonly detail: string;
    readonly category: string;
    /** Stable contribution namespace, such as `core` or `project.tasks`. */
    readonly source: string;
    /** `null` means intentionally unbound. */
    readonly defaultBinding: string | null;
    /** Controls palette/keymap discovery. Hidden contextual matches fall through. */
    readonly when?: ActionPredicate;
    /** Controls dispatch. A visible disabled match shadows less-specific matches. */
    readonly enabled?: ActionPredicate;
    readonly run: (context: ActionContext) => Result | PromiseLike<Result>;
}

export interface ActionPrecedenceMetadata {
    readonly scope: ActionScopeKind;
    /** Higher values are more specific. */
    readonly rank: number;
    readonly targetId: string | null;
    readonly matchingContributions: number;
    readonly fallbackDepth: number;
    readonly shadowedContributions: number;
}

export interface ResolvedAction<Result = unknown> {
    readonly contributionId: ActionContributionId;
    readonly definition: ActionDefinition<Result>;
    readonly visible: boolean;
    readonly enabled: boolean;
    readonly precedence: ActionPrecedenceMetadata;
    readonly contextFingerprint: string;
}

export interface ActionContributionMetadata<Result = unknown> {
    readonly contributionId: ActionContributionId;
    readonly definition: ActionDefinition<Result>;
    readonly scope: ActionScope;
    readonly registrationOrder: number;
}

export interface ActionRegistrationOptions {
    readonly scope?: ActionScope;
    /** Synchronous contribution cleanup, invoked after removal. */
    readonly onDispose?: () => void;
}

export interface ActionRegistration<Result = unknown> extends ActionContributionMetadata<Result> {
    readonly disposed: boolean;
    dispose(): void;
}

export const ACTION_SCOPE_PRECEDENCE = Object.freeze(["focused-item", "session", "project", "global"] as const);

export const ACTION_REGISTRY_LIMITS = Object.freeze({
    maxRegistrations: 4_096,
    maxIdLength: 128,
    maxEntityIdLength: 256,
    maxTitleLength: 256,
    maxDetailLength: 2_048,
    maxCategoryLength: 128,
    maxBindingLength: 256,
    maxProjectRootLength: 4_096,
    maxCapabilities: 128,
});

export class DuplicateActionContributionError extends Error {
    constructor(
        readonly actionId: string,
        readonly scope: ActionScopeKind,
        readonly targetId: string | null,
        readonly existingContributionId: ActionContributionId,
        readonly incomingContributionId: ActionContributionId,
    ) {
        super(`Action ${actionId} already has a ${scope} contribution for ${targetId ?? "the global scope"}`);
        this.name = "DuplicateActionContributionError";
    }
}

export class ActionRegistryDisposedError extends Error {
    constructor() {
        super("Action registry has been disposed");
        this.name = "ActionRegistryDisposedError";
    }
}

export class ActionNotFoundError extends Error {
    constructor(readonly actionId: string) {
        super(`No matching action is registered: ${actionId}`);
        this.name = "ActionNotFoundError";
    }
}

export class ActionNotVisibleError extends Error {
    constructor(readonly actionId: string) {
        super(`Action is not visible in this context: ${actionId}`);
        this.name = "ActionNotVisibleError";
    }
}

export class ActionDisabledError extends Error {
    constructor(readonly actionId: string) {
        super(`Action is disabled in this context: ${actionId}`);
        this.name = "ActionDisabledError";
    }
}

type RegisteredAction = {
    readonly contributionId: ActionContributionId;
    readonly definition: ActionDefinition<unknown>;
    readonly scope: ActionScope;
    readonly slotKey: string;
    readonly registrationOrder: number;
    readonly onDispose?: () => void;
    active: boolean;
};

const UNSAFE_RECORD_KEYS = new Set(["__proto__", "constructor", "prototype"]);
const SYMBOL_PATTERN = /^[a-z][A-Za-z0-9]*(?:[._-][A-Za-z0-9]+)*$/;
const SCOPE_RANK: Readonly<Record<ActionScopeKind, number>> = Object.freeze({
    global: 0,
    project: 1,
    session: 2,
    "focused-item": 3,
});
const GLOBAL_SCOPE: ActionScope = Object.freeze({ kind: "global" });

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function containsUnsafeNamespaceSegment(value: string): boolean {
    return value.split(/[._-]/u).some((segment) => UNSAFE_RECORD_KEYS.has(segment));
}

function requireSymbol(name: string, value: unknown, maxLength = ACTION_REGISTRY_LIMITS.maxIdLength): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > maxLength ||
        value !== value.trim() ||
        containsControlCharacter(value) ||
        containsUnsafeNamespaceSegment(value) ||
        !SYMBOL_PATTERN.test(value)
    ) {
        throw new TypeError(`${name} must be a bounded, record-safe identifier without control characters`);
    }
    return value;
}

function requireEntityId(name: string, value: unknown): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > ACTION_REGISTRY_LIMITS.maxEntityIdLength ||
        value !== value.trim() ||
        containsControlCharacter(value) ||
        UNSAFE_RECORD_KEYS.has(value)
    ) {
        throw new TypeError(`${name} must be a bounded, trimmed, record-safe ID without control characters`);
    }
    return value;
}

function requireText(name: string, value: unknown, maxLength: number, allowEmpty: boolean): string {
    if (
        typeof value !== "string" ||
        value.length > maxLength ||
        value !== value.trim() ||
        containsControlCharacter(value) ||
        (!allowEmpty && value.length === 0) ||
        UNSAFE_RECORD_KEYS.has(value)
    ) {
        throw new TypeError(`${name} must be bounded, trimmed, record-safe text without control characters`);
    }
    return value;
}

function copyItem(value: ActionItemContext | null | undefined): ActionItemContext | null {
    if (value == null) return null;
    return Object.freeze({
        id: requireEntityId("focused item ID", value.id),
        kind: requireSymbol("focused item kind", value.kind),
    });
}

function copySession(value: ActionSessionContext | null | undefined): ActionSessionContext | null {
    if (value == null) return null;
    return Object.freeze({
        id: requireEntityId("session ID", value.id),
        kind: requireSymbol("session kind", value.kind),
    });
}

function copyProject(value: ActionProjectContext | null | undefined): ActionProjectContext | null {
    if (value == null) return null;
    const root = value.root;
    if (
        typeof root !== "string" ||
        root.length === 0 ||
        root.length > ACTION_REGISTRY_LIMITS.maxProjectRootLength ||
        root.trim().length === 0 ||
        containsControlCharacter(root)
    ) {
        throw new TypeError("project root must be bounded, non-blank text without control characters");
    }
    return Object.freeze({ id: requireEntityId("project ID", value.id), root });
}

function copyFocus(value: ActionFocusContext | undefined): ActionFocusContext {
    if (value === undefined) return Object.freeze({ target: "application", editable: false });
    if (typeof value.editable !== "boolean") throw new TypeError("focus editable must be boolean");
    return Object.freeze({ target: requireSymbol("focus target", value.target), editable: value.editable });
}

function copyModal(value: ActionModalContext | null | undefined): ActionModalContext | null {
    if (value == null) return null;
    return Object.freeze({
        id: requireEntityId("modal ID", value.id),
        kind: requireSymbol("modal kind", value.kind),
    });
}

function copyAgent(value: ActionAgentContext | null | undefined): ActionAgentContext | null {
    if (value == null) return null;
    return Object.freeze({
        id: requireEntityId("agent ID", value.id),
        kind: requireSymbol("agent kind", value.kind),
        status: requireSymbol("agent status", value.status),
    });
}

function copyCapabilities(values: Iterable<string> | undefined): readonly string[] {
    if (values === undefined) return Object.freeze([]);
    const capabilities = new Set<string>();
    let count = 0;
    for (const value of values) {
        count += 1;
        if (count > ACTION_REGISTRY_LIMITS.maxCapabilities) {
            throw new RangeError(`action contexts cannot contain more than ${ACTION_REGISTRY_LIMITS.maxCapabilities} capabilities`);
        }
        capabilities.add(requireSymbol("action capability", value));
    }
    return Object.freeze(Array.from(capabilities).sort());
}

export function createActionContext(input: ActionContextInput = {}): ActionContext {
    return Object.freeze({
        focusedItem: copyItem(input.focusedItem),
        session: copySession(input.session),
        project: copyProject(input.project),
        focus: copyFocus(input.focus),
        modal: copyModal(input.modal),
        agent: copyAgent(input.agent),
        capabilities: copyCapabilities(input.capabilities),
    });
}

function fingerprintCanonicalActionContext(context: ActionContext): string {
    return JSON.stringify([
        "action-context-v1",
        context.focusedItem?.id ?? null,
        context.focusedItem?.kind ?? null,
        context.session?.id ?? null,
        context.session?.kind ?? null,
        context.project?.id ?? null,
        context.project?.root ?? null,
        context.focus.target,
        context.focus.editable,
        context.modal?.id ?? null,
        context.modal?.kind ?? null,
        context.agent?.id ?? null,
        context.agent?.kind ?? null,
        context.agent?.status ?? null,
        context.capabilities,
    ]);
}

/**
 * Stable fingerprint of the bounded scalar context. The registry deliberately
 * does not cache predicate results: `when` and `enabled` may close over live
 * state that is not represented here. Callers may memoize only pure predicates.
 */
export function fingerprintActionContext(input: ActionContextInput): string {
    return fingerprintCanonicalActionContext(createActionContext(input));
}

function validateDefinition<Result>(definition: ActionDefinition<Result>): ActionDefinition<Result> {
    if (typeof definition !== "object" || definition === null) throw new TypeError("action definitions must be objects");
    const defaultBinding =
        definition.defaultBinding === null
            ? null
            : requireText("action default binding", definition.defaultBinding, ACTION_REGISTRY_LIMITS.maxBindingLength, false);
    if (definition.when !== undefined && typeof definition.when !== "function") throw new TypeError("action when must be a function");
    if (definition.enabled !== undefined && typeof definition.enabled !== "function") throw new TypeError("action enabled must be a function");
    if (typeof definition.run !== "function") throw new TypeError("action run must be a function");

    return Object.freeze({
        id: requireSymbol("action ID", definition.id),
        title: requireText("action title", definition.title, ACTION_REGISTRY_LIMITS.maxTitleLength, false),
        detail: requireText("action detail", definition.detail, ACTION_REGISTRY_LIMITS.maxDetailLength, true),
        category: requireText("action category", definition.category, ACTION_REGISTRY_LIMITS.maxCategoryLength, false),
        source: requireSymbol("action source", definition.source),
        defaultBinding,
        when: definition.when,
        enabled: definition.enabled,
        run: definition.run,
    });
}

function validateScope(scope: ActionScope | undefined): ActionScope {
    if (scope === undefined || scope.kind === "global") return GLOBAL_SCOPE;
    if (scope.kind !== "project" && scope.kind !== "session" && scope.kind !== "focused-item") {
        throw new TypeError(`Unknown action scope: ${String((scope as { readonly kind?: unknown }).kind)}`);
    }
    return Object.freeze({ kind: scope.kind, id: requireEntityId(`${scope.kind} scope ID`, scope.id) });
}

function scopeTargetId(scope: ActionScope): string | null {
    return scope.kind === "global" ? null : scope.id;
}

function scopeSlotKey(actionId: string, scope: ActionScope): string {
    return JSON.stringify([actionId, scope.kind, scopeTargetId(scope)]);
}

function contributionIdFor(definition: ActionDefinition<unknown>, scope: ActionScope): ActionContributionId {
    const target = scopeTargetId(scope);
    const suffix = target === null ? scope.kind : `${scope.kind}:${JSON.stringify(target)}`;
    return `internal:${definition.source}/${definition.id}@${suffix}` as ActionContributionId;
}

function scopeMatches(scope: ActionScope, context: ActionContext): boolean {
    switch (scope.kind) {
        case "global":
            return true;
        case "project":
            return context.project?.id === scope.id;
        case "session":
            return context.session?.id === scope.id;
        case "focused-item":
            return context.focusedItem?.id === scope.id;
    }
}

function predicateValue(name: "when" | "enabled", predicate: ActionPredicate | undefined, context: ActionContext): boolean {
    if (!predicate) return true;
    const value = predicate(context);
    if (typeof value !== "boolean") throw new TypeError(`action ${name} predicates must return boolean`);
    return value;
}

function metadataFor(record: RegisteredAction): ActionContributionMetadata {
    return Object.freeze({
        contributionId: record.contributionId,
        definition: record.definition,
        scope: record.scope,
        registrationOrder: record.registrationOrder,
    });
}

export class ActionRegistry {
    private readonly byContribution = new Map<ActionContributionId, RegisteredAction>();
    private readonly bySlot = new Map<string, RegisteredAction>();
    private readonly byAction = new Map<string, RegisteredAction[]>();
    private nextRegistrationOrder = 1;
    private disposed = false;

    get size(): number {
        return this.byContribution.size;
    }

    get isDisposed(): boolean {
        return this.disposed;
    }

    register<Result>(definitionInput: ActionDefinition<Result>, options: ActionRegistrationOptions = {}): ActionRegistration<Result> {
        if (this.disposed) throw new ActionRegistryDisposedError();
        if (this.size >= ACTION_REGISTRY_LIMITS.maxRegistrations) {
            throw new RangeError(`action registry cannot exceed ${ACTION_REGISTRY_LIMITS.maxRegistrations} contributions`);
        }
        if (options.onDispose !== undefined && typeof options.onDispose !== "function") {
            throw new TypeError("action registration onDispose must be a function");
        }

        const definition = validateDefinition(definitionInput);
        const scope = validateScope(options.scope);
        const slotKey = scopeSlotKey(definition.id, scope);
        const contributionId = contributionIdFor(definition as ActionDefinition<unknown>, scope);
        const existing = this.bySlot.get(slotKey);
        if (existing) {
            throw new DuplicateActionContributionError(definition.id, scope.kind, scopeTargetId(scope), existing.contributionId, contributionId);
        }

        const record: RegisteredAction = {
            contributionId,
            definition: definition as ActionDefinition<unknown>,
            scope,
            slotKey,
            registrationOrder: this.nextRegistrationOrder,
            onDispose: options.onDispose,
            active: true,
        };
        this.nextRegistrationOrder += 1;
        this.byContribution.set(contributionId, record);
        this.bySlot.set(slotKey, record);
        const actionRecords = this.byAction.get(definition.id);
        if (actionRecords) actionRecords.push(record);
        else this.byAction.set(definition.id, [record]);

        return Object.freeze({
            ...metadataFor(record),
            get disposed() {
                return !record.active;
            },
            dispose: () => this.unregister(record),
        }) as ActionRegistration<Result>;
    }

    contributions(): readonly ActionContributionMetadata[] {
        return Object.freeze(
            Array.from(this.byContribution.values())
                .sort((left, right) => left.registrationOrder - right.registrationOrder)
                .map(metadataFor),
        );
    }

    resolveAction(actionId: string, input: ActionContextInput): ResolvedAction | undefined {
        const id = requireSymbol("action ID", actionId);
        const context = createActionContext(input);
        return this.resolveCanonical(id, context, fingerprintCanonicalActionContext(context));
    }

    resolve(input: ActionContextInput): readonly ResolvedAction[] {
        const context = createActionContext(input);
        const fingerprint = fingerprintCanonicalActionContext(context);
        const resolved: { readonly value: ResolvedAction; readonly order: number }[] = [];
        for (const actionId of this.byAction.keys()) {
            const value = this.resolveCanonical(actionId, context, fingerprint);
            if (!value?.visible) continue;
            const selected = this.byContribution.get(value.contributionId);
            if (selected) resolved.push({ value, order: selected.registrationOrder });
        }
        resolved.sort((left, right) => left.order - right.order || left.value.definition.id.localeCompare(right.value.definition.id));
        return Object.freeze(resolved.map(({ value }) => value));
    }

    async execute<Result = unknown>(actionId: string, input: ActionContextInput): Promise<Awaited<Result>> {
        const id = requireSymbol("action ID", actionId);
        const context = createActionContext(input);
        const resolution = this.resolveCanonical(id, context, fingerprintCanonicalActionContext(context));
        if (!resolution) throw new ActionNotFoundError(id);
        if (!resolution.visible) throw new ActionNotVisibleError(id);
        if (!resolution.enabled) throw new ActionDisabledError(id);
        return (await resolution.definition.run(context)) as Awaited<Result>;
    }

    /** Removes every live contribution in reverse registration order. */
    dispose(): void {
        if (this.disposed) return;
        this.disposed = true;
        const errors: unknown[] = [];
        const records = Array.from(this.byContribution.values()).sort((left, right) => right.registrationOrder - left.registrationOrder);
        for (const record of records) {
            try {
                this.unregister(record);
            } catch (error) {
                errors.push(error);
            }
        }
        if (errors.length > 0) throw new AggregateError(errors, "One or more action contribution teardown callbacks failed");
    }

    private resolveCanonical(actionId: string, context: ActionContext, contextFingerprint: string): ResolvedAction | undefined {
        const candidates = (this.byAction.get(actionId) ?? [])
            .filter((record) => record.active && scopeMatches(record.scope, context))
            .sort((left, right) => SCOPE_RANK[right.scope.kind] - SCOPE_RANK[left.scope.kind] || left.registrationOrder - right.registrationOrder);
        if (candidates.length === 0) return undefined;

        let selected = candidates[0];
        let fallbackDepth = candidates.length;
        let visible = false;
        for (let index = 0; index < candidates.length; index += 1) {
            const candidate = candidates[index];
            if (!predicateValue("when", candidate.definition.when, context)) continue;
            selected = candidate;
            fallbackDepth = index;
            visible = true;
            break;
        }
        const enabled = visible && predicateValue("enabled", selected.definition.enabled, context);
        return Object.freeze({
            contributionId: selected.contributionId,
            definition: selected.definition,
            visible,
            enabled,
            precedence: Object.freeze({
                scope: selected.scope.kind,
                rank: SCOPE_RANK[selected.scope.kind],
                targetId: scopeTargetId(selected.scope),
                matchingContributions: candidates.length,
                fallbackDepth,
                shadowedContributions: visible ? candidates.length - fallbackDepth - 1 : 0,
            }),
            contextFingerprint,
        });
    }

    private unregister(record: RegisteredAction): void {
        if (!record.active) return;
        record.active = false;
        this.byContribution.delete(record.contributionId);
        this.bySlot.delete(record.slotKey);
        const records = this.byAction.get(record.definition.id);
        if (records) {
            const index = records.indexOf(record);
            if (index >= 0) records.splice(index, 1);
            if (records.length === 0) this.byAction.delete(record.definition.id);
        }
        record.onDispose?.();
    }
}
