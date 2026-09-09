import type { PaneKind, PaneNode } from "../state/types/domain";
import type { EditorPaneView } from "../state/types/view";

declare const ITEM_ID_BRAND: unique symbol;

export type ItemId = string & { readonly [ITEM_ID_BRAND]: "ItemId" };

export interface WorkbenchItemRef<K extends string = PaneKind> {
    readonly id: ItemId;
    readonly kind: K;
}

export interface WorkbenchItemController {
    activate(): void | Promise<void>;
    deactivate(): void | Promise<void>;
    canClose(): boolean | Promise<boolean>;
    dispose(): void | Promise<void>;
}

export type PersistedCodecResult<State> = { readonly ok: true; readonly value: State } | { readonly ok: false };

export interface VersionedPersistedCodec<State> {
    readonly version: number;
    encode(state: State): unknown;
    decode(encoded: unknown): PersistedCodecResult<State>;
}

export interface WorkbenchItemDefinition<Kind extends string = string, State = unknown> {
    readonly kind: Kind;
    readonly defaultTitle: string;
    readonly create: (ref: WorkbenchItemRef<Kind>) => WorkbenchItemController;
    readonly persisted: VersionedPersistedCodec<State>;
    readonly cleanupDraft?: (state: State) => void | Promise<void>;
}

export interface BuiltinWorkbenchItemState {
    terminal: null;
    editor: EditorPaneView;
    git: null;
    diff: null;
    aws: null;
    search: null;
    rundeck: null;
    bruno: null;
}

/**
 * Persisted editor state is intentionally bounded. Paths use UTF-16 code-unit
 * length (JavaScript's string length), must be non-blank, unique, and free of
 * C0/C1 controls. A selection must name an open tab.
 */
export const EDITOR_PERSISTENCE_LIMITS = Object.freeze({
    maxOpenTabs: 128,
    maxPathLength: 4_096,
});

export interface PersistedWorkbenchItemEnvelope<Kind extends PaneKind = PaneKind> {
    readonly itemId: string;
    readonly kind: Kind;
    readonly version: number;
    readonly state: unknown;
}

export type PersistedItemFailureReason =
    "invalid-envelope" | "unknown-kind" | "item-id-mismatch" | "kind-mismatch" | "version-mismatch" | "invalid-state";

export type DecodePersistedItemResult<Kind extends PaneKind> =
    | {
          readonly ok: true;
          readonly ref: WorkbenchItemRef<Kind>;
          readonly state: BuiltinWorkbenchItemState[Kind];
      }
    | { readonly ok: false; readonly reason: PersistedItemFailureReason };

export class UnknownWorkbenchItemKindError extends Error {
    constructor(readonly kind: string) {
        super(`Unknown workbench item kind: ${kind}`);
        this.name = "UnknownWorkbenchItemKindError";
    }
}

export class DuplicateWorkbenchItemKindError extends Error {
    constructor(readonly kind: string) {
        super(`Workbench item kind is already registered: ${kind}`);
        this.name = "DuplicateWorkbenchItemKindError";
    }
}

const CODEC_FAILURE = Object.freeze({ ok: false }) as PersistedCodecResult<never>;
const ITEM_ID_MAX_LENGTH = 256;
const KIND_PATTERN = /^[a-z][a-z0-9._:-]*$/;
const ENVELOPE_KEYS = new Set<PropertyKey>(["itemId", "kind", "version", "state"]);
const UNSAFE_RECORD_KEYS = new Set(["__proto__", "constructor", "prototype"]);

function isRecord(value: unknown): value is Record<PropertyKey, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function containsControlCharacter(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

export function isValidWorkbenchItemId(value: unknown): value is string {
    return (
        typeof value === "string" &&
        value.length > 0 &&
        value.length <= ITEM_ID_MAX_LENGTH &&
        value === value.trim() &&
        !UNSAFE_RECORD_KEYS.has(value) &&
        !containsControlCharacter(value)
    );
}

function isValidKind(value: unknown): value is string {
    return typeof value === "string" && value.length <= 128 && KIND_PATTERN.test(value);
}

export function createItemId(value: string): ItemId {
    if (!isValidWorkbenchItemId(value)) {
        throw new TypeError("workbench item IDs must be bounded, non-empty, trimmed, record-safe, and free of control characters");
    }
    return value as ItemId;
}

export function createWorkbenchItemRef<Kind extends string>(id: string, kind: Kind): WorkbenchItemRef<Kind> {
    if (!isValidKind(kind)) throw new TypeError(`Invalid workbench item kind: ${String(kind)}`);
    return Object.freeze({ id: createItemId(id), kind });
}

export function workbenchItemRefFromPane(pane: Pick<PaneNode, "id" | "kind">): WorkbenchItemRef<PaneKind> {
    return createWorkbenchItemRef(pane.id, pane.kind);
}

export function createNoopWorkbenchItemController(): WorkbenchItemController {
    return Object.freeze({
        activate: () => {},
        deactivate: () => {},
        canClose: () => true,
        dispose: () => {},
    });
}

function nullCodec(): VersionedPersistedCodec<null> {
    return Object.freeze({
        version: 1,
        encode: (state: null) => {
            if (state !== null) throw new TypeError("This workbench item does not persist item-specific state");
            return null;
        },
        decode: (encoded: unknown): PersistedCodecResult<null> => (encoded === null ? { ok: true, value: null } : CODEC_FAILURE),
    });
}

function isValidPersistedEditorPath(value: unknown): value is string {
    return (
        typeof value === "string" &&
        value.length > 0 &&
        value.length <= EDITOR_PERSISTENCE_LIMITS.maxPathLength &&
        value.trim().length > 0 &&
        !containsControlCharacter(value)
    );
}

function decodeEditorView(encoded: unknown): PersistedCodecResult<EditorPaneView> {
    if (!isRecord(encoded)) return CODEC_FAILURE;
    const { openTabs, activePath } = encoded;
    if (!Array.isArray(openTabs) || openTabs.length > EDITOR_PERSISTENCE_LIMITS.maxOpenTabs) return CODEC_FAILURE;
    const uniquePaths = new Set<string>();
    for (const path of openTabs) {
        if (!isValidPersistedEditorPath(path) || uniquePaths.has(path)) return CODEC_FAILURE;
        uniquePaths.add(path);
    }
    if (activePath !== null && (!isValidPersistedEditorPath(activePath) || !uniquePaths.has(activePath))) return CODEC_FAILURE;
    return { ok: true, value: { openTabs: openTabs.slice(), activePath } };
}

const EDITOR_CODEC: VersionedPersistedCodec<EditorPaneView> = Object.freeze({
    version: 2,
    encode: (state: EditorPaneView) => {
        const decoded = decodeEditorView(state);
        if (!decoded.ok) throw new TypeError("Invalid editor workbench state");
        return decoded.value;
    },
    decode: decodeEditorView,
});

const NULL_CODEC = nullCodec();

function builtinDefinition<Kind extends PaneKind, State>(
    kind: Kind,
    defaultTitle: string,
    persisted: VersionedPersistedCodec<State>,
): WorkbenchItemDefinition<Kind, State> {
    return Object.freeze({
        kind,
        defaultTitle,
        persisted,
        create: (_ref: WorkbenchItemRef<Kind>) => createNoopWorkbenchItemController(),
        cleanupDraft: (_state: State) => {},
    });
}

type BuiltinDefinitionMap = {
    readonly [Kind in PaneKind]: WorkbenchItemDefinition<Kind, BuiltinWorkbenchItemState[Kind]>;
};

/** This keyed shape intentionally makes additions to PaneKind a compile-time exhaustiveness error. */
export const BUILTIN_WORKBENCH_ITEM_MANIFEST = Object.freeze({
    terminal: builtinDefinition("terminal", "shell", NULL_CODEC),
    editor: builtinDefinition("editor", "editor", EDITOR_CODEC),
    git: builtinDefinition("git", "git", NULL_CODEC),
    diff: builtinDefinition("diff", "diff", NULL_CODEC),
    aws: builtinDefinition("aws", "aws", NULL_CODEC),
    search: builtinDefinition("search", "search", NULL_CODEC),
    rundeck: builtinDefinition("rundeck", "rundeck", NULL_CODEC),
    bruno: builtinDefinition("bruno", "bruno", NULL_CODEC),
}) satisfies BuiltinDefinitionMap;

export function defaultWorkbenchItemTitle(kind: PaneKind, startup?: string): string {
    return kind === "terminal" && startup ? startup : BUILTIN_WORKBENCH_ITEM_MANIFEST[kind].defaultTitle;
}

const BUILTIN_KIND_SET = new Set<PaneKind>(Object.keys(BUILTIN_WORKBENCH_ITEM_MANIFEST) as PaneKind[]);

export function isBuiltinWorkbenchItemKind(value: unknown): value is PaneKind {
    return typeof value === "string" && BUILTIN_KIND_SET.has(value as PaneKind);
}

function builtinDefinitionFor<Kind extends PaneKind>(kind: Kind): WorkbenchItemDefinition<Kind, BuiltinWorkbenchItemState[Kind]> {
    return BUILTIN_WORKBENCH_ITEM_MANIFEST[kind] as unknown as WorkbenchItemDefinition<Kind, BuiltinWorkbenchItemState[Kind]>;
}

function decodeFailure<Kind extends PaneKind>(reason: PersistedItemFailureReason): DecodePersistedItemResult<Kind> {
    return { ok: false, reason };
}

function readEnvelope(encoded: unknown): PersistedWorkbenchItemEnvelope | null {
    if (!isRecord(encoded)) return null;
    try {
        const keys = Reflect.ownKeys(encoded);
        if (keys.length !== ENVELOPE_KEYS.size || keys.some((key) => !ENVELOPE_KEYS.has(key))) return null;
        const values = Object.fromEntries(
            keys.map((key) => {
                const descriptor = Object.getOwnPropertyDescriptor(encoded, key);
                if (!descriptor || !("value" in descriptor)) throw new TypeError("Persisted item envelopes cannot contain accessors");
                return [key, descriptor.value];
            }),
        );
        if (!isValidWorkbenchItemId(values.itemId)) return null;
        if (typeof values.kind !== "string") return null;
        if (!Number.isSafeInteger(values.version) || (values.version as number) <= 0) return null;
        return {
            itemId: values.itemId,
            kind: values.kind as PaneKind,
            version: values.version as number,
            state: values.state,
        };
    } catch {
        return null;
    }
}

type ErasedDefinition = WorkbenchItemDefinition<string, unknown>;

function eraseDefinition<Kind extends string, State>(definition: WorkbenchItemDefinition<Kind, State>): ErasedDefinition {
    const create = definition.create;
    const encode = definition.persisted.encode;
    const decode = definition.persisted.decode;
    const cleanupDraft = definition.cleanupDraft;
    return Object.freeze({
        kind: definition.kind,
        defaultTitle: definition.defaultTitle.trim(),
        create: (ref: WorkbenchItemRef<string>) => create(ref as WorkbenchItemRef<Kind>),
        persisted: Object.freeze({
            version: definition.persisted.version,
            encode: (state: unknown) => encode(state as State),
            decode: (encoded: unknown): PersistedCodecResult<unknown> => decode(encoded),
        }),
        cleanupDraft: cleanupDraft ? (state: unknown) => cleanupDraft(state as State) : undefined,
    });
}

function isController(value: unknown): value is WorkbenchItemController {
    if (!isRecord(value)) return false;
    return [value.activate, value.deactivate, value.canClose, value.dispose].every((method) => typeof method === "function");
}

export class WorkbenchItemRegistry {
    private readonly definitions = new Map<string, ErasedDefinition>();

    constructor() {
        for (const kind of Object.keys(BUILTIN_WORKBENCH_ITEM_MANIFEST) as PaneKind[]) {
            this.register(builtinDefinitionFor(kind));
        }
    }

    register<Kind extends string, State>(definition: WorkbenchItemDefinition<Kind, State>): void {
        if (!isValidKind(definition.kind)) throw new TypeError(`Invalid workbench item kind: ${String(definition.kind)}`);
        if (this.definitions.has(definition.kind)) throw new DuplicateWorkbenchItemKindError(definition.kind);
        if (!definition.defaultTitle.trim()) throw new TypeError("Workbench item definitions require a default title");
        if (typeof definition.create !== "function") throw new TypeError("Workbench item definitions require a controller factory");
        if (!Number.isSafeInteger(definition.persisted.version) || definition.persisted.version <= 0) {
            throw new TypeError("Workbench item persisted codec versions must be positive integers");
        }
        if (typeof definition.persisted.encode !== "function" || typeof definition.persisted.decode !== "function") {
            throw new TypeError("Workbench item definitions require a persisted codec");
        }
        if (definition.cleanupDraft !== undefined && typeof definition.cleanupDraft !== "function") {
            throw new TypeError("Workbench item cleanupDraft must be a function");
        }
        this.definitions.set(definition.kind, eraseDefinition(definition));
    }

    has(kind: string): boolean {
        return this.definitions.has(kind);
    }

    kinds(): readonly string[] {
        return Array.from(this.definitions.keys());
    }

    get(kind: string): ErasedDefinition {
        const definition = this.definitions.get(kind);
        if (!definition) throw new UnknownWorkbenchItemKindError(kind);
        return definition;
    }

    create<Kind extends string>(ref: WorkbenchItemRef<Kind>): WorkbenchItemController {
        if (!isValidWorkbenchItemId(ref.id)) throw new TypeError("Invalid workbench item ID");
        const controller = this.get(ref.kind).create(ref);
        if (!isController(controller)) throw new TypeError(`Controller factory for ${ref.kind} returned an invalid controller`);
        return controller;
    }

    async cleanupDraft<Kind extends string>(ref: WorkbenchItemRef<Kind>, state: unknown): Promise<void> {
        const cleanup = this.get(ref.kind).cleanupDraft;
        if (cleanup) await cleanup(state);
    }

    encodePersisted<Kind extends PaneKind>(
        ref: WorkbenchItemRef<Kind>,
        state: BuiltinWorkbenchItemState[Kind],
    ): PersistedWorkbenchItemEnvelope<Kind> {
        if (!isValidWorkbenchItemId(ref.id)) throw new TypeError("Invalid workbench item ID");
        if (!isBuiltinWorkbenchItemKind(ref.kind)) throw new UnknownWorkbenchItemKindError(ref.kind);
        const definition = builtinDefinitionFor(ref.kind);
        return Object.freeze({
            itemId: ref.id,
            kind: ref.kind,
            version: definition.persisted.version,
            state: definition.persisted.encode(state),
        });
    }

    decodePersisted<Kind extends PaneKind>(expected: WorkbenchItemRef<Kind>, encoded: unknown): DecodePersistedItemResult<Kind> {
        if (!isValidWorkbenchItemId(expected.id)) return decodeFailure("invalid-envelope");
        if (!isBuiltinWorkbenchItemKind(expected.kind)) return decodeFailure("unknown-kind");
        const envelope = readEnvelope(encoded);
        if (!envelope) return decodeFailure("invalid-envelope");
        if (!isBuiltinWorkbenchItemKind(envelope.kind)) return decodeFailure("unknown-kind");
        if (envelope.itemId !== expected.id) return decodeFailure("item-id-mismatch");
        if (envelope.kind !== expected.kind) return decodeFailure("kind-mismatch");

        const definition = builtinDefinitionFor(expected.kind);
        if (envelope.version !== definition.persisted.version) return decodeFailure("version-mismatch");
        try {
            const decoded = definition.persisted.decode(envelope.state);
            if (!decoded.ok) return decodeFailure("invalid-state");
            return { ok: true, ref: expected, state: decoded.value };
        } catch {
            return decodeFailure("invalid-state");
        }
    }
}

export const workbenchItemRegistry = new WorkbenchItemRegistry();
