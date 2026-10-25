import { isPluginKind } from "../plugins/kinds";
import { pluginSurface } from "../plugins/registry";
import type { PaneKind } from "../state/types/domain";
import { CORE_PANE_KINDS, isCorePaneKind } from "../state/windowRoles";
import { isRecord } from "@shared/lib/guards";
import type { DeskBrowserTab, DeskSavedSimulator, DeskView, EditorPaneView } from "../state/types/view";

/** Saved editor state is bounded, and a selection must name an open tab. */
export const EDITOR_PERSISTENCE_LIMITS = Object.freeze({
    maxOpenTabs: 128,
    maxPathLength: 4_096,
});

/**
 * A saved desk is bounded the same way, and only ordinary web pages come back:
 * a page reopens itself on restore, so a scheme read off disk must not be one
 * that can reach the machine or run on its own.
 */
export const DESK_PERSISTENCE_LIMITS = Object.freeze({
    maxTabs: 24,
    maxFiles: 24,
    maxUrlLength: 4_096,
    maxTitleLength: 200,
});

const ITEM_ID_MAX_LENGTH = 256;
const UNSAFE_RECORD_KEYS = new Set(["__proto__", "constructor", "prototype"]);

/** A pane id read off disk, which keys the store's records. */
export function isValidWorkbenchItemId(value: unknown): value is string {
    return (
        typeof value === "string" &&
        value.length > 0 &&
        value.length <= ITEM_ID_MAX_LENGTH &&
        value === value.trim() &&
        !UNSAFE_RECORD_KEYS.has(value)
    );
}

/** Plugin kinds are accepted whether or not the plugin is in this build, so its panes survive a build without it. */
export function isWorkbenchItemKind(value: unknown): value is PaneKind {
    return isCorePaneKind(value) || isPluginKind(value);
}

export function defaultWorkbenchItemTitle(kind: PaneKind, startup?: string): string {
    if (isPluginKind(kind)) return pluginSurface(kind)?.title ?? kind;
    return kind === "terminal" && startup ? startup : CORE_PANE_KINDS[kind].title;
}

const isSavedPath = (value: unknown): value is string =>
    typeof value === "string" && value.trim().length > 0 && value.length <= EDITOR_PERSISTENCE_LIMITS.maxPathLength;

function decodeEditorView(encoded: unknown): EditorPaneView | null {
    if (!isRecord(encoded)) return null;
    const { openTabs, activePath, single, preview } = encoded;
    if (!Array.isArray(openTabs) || openTabs.length > EDITOR_PERSISTENCE_LIMITS.maxOpenTabs) return null;
    const open = new Set<string>();
    for (const path of openTabs) {
        if (!isSavedPath(path) || open.has(path)) return null;
        open.add(path);
    }
    if (activePath !== null && (!isSavedPath(activePath) || !open.has(activePath))) return null;
    if (single !== undefined && single !== true) return null;
    if (preview !== undefined && typeof preview !== "string") return null;
    return { openTabs: [...open], activePath, ...(single ? { single } : {}), ...(preview !== undefined && open.has(preview) ? { preview } : {}) };
}

const isWebPage = (value: unknown): value is string =>
    typeof value === "string" && value.length <= DESK_PERSISTENCE_LIMITS.maxUrlLength && /^https?:\/\/./i.test(value);

const isSavedLabel = (value: unknown): value is string => typeof value === "string" && value.length <= DESK_PERSISTENCE_LIMITS.maxTitleLength;

function decodeDeskSimulator(encoded: unknown): DeskSavedSimulator | null {
    if (!isRecord(encoded)) return null;
    const { udid, name } = encoded;
    return (udid === null || isSavedLabel(udid)) && (name === null || isSavedLabel(name)) ? { udid, name } : null;
}

function decodeDeskView(encoded: unknown): DeskView | null {
    if (!isRecord(encoded)) return null;
    const { agentId, tabs, activeIndex, files } = encoded;
    if (!isValidWorkbenchItemId(agentId)) return null;
    if (!Array.isArray(tabs) || tabs.length > DESK_PERSISTENCE_LIMITS.maxTabs) return null;
    if (!Array.isArray(files) || files.length > DESK_PERSISTENCE_LIMITS.maxFiles) return null;
    const simulator = encoded.simulator === undefined ? undefined : decodeDeskSimulator(encoded.simulator);
    if (simulator === null) return null;
    if (tabs.length === 0 && files.length === 0 && !simulator) return null;
    const restored: DeskBrowserTab[] = [];
    for (const tab of tabs) {
        if (!isRecord(tab) || !isWebPage(tab.url) || !isSavedLabel(tab.title)) return null;
        restored.push({ url: tab.url, title: tab.title });
    }
    if (!Number.isInteger(activeIndex) || (activeIndex as number) < 0 || (activeIndex as number) >= Math.max(1, restored.length)) return null;
    const unique = new Set<string>();
    for (const path of files) {
        if (!isSavedPath(path) || unique.has(path)) return null;
        unique.add(path);
    }
    return { agentId, tabs: restored, activeIndex: activeIndex as number, files: [...unique], ...(simulator ? { simulator } : {}) };
}

/** The panes that keep state of their own across a restart: the version it is saved at, and how a saved one reads back. */
const ITEM_STATES = {
    editor: { version: 2, decode: decodeEditorView },
    desk: { version: 1, decode: decodeDeskView },
} as const;

export interface SavedItemStates {
    editor: EditorPaneView;
    desk: DeskView;
}

export type SavedItemKind = keyof SavedItemStates;

export interface PersistedWorkbenchItemEnvelope {
    readonly itemId: string;
    readonly kind: SavedItemKind;
    readonly version: number;
    readonly state: unknown;
}

export function encodeItemState<Kind extends SavedItemKind>(
    itemId: string,
    kind: Kind,
    state: SavedItemStates[Kind],
): PersistedWorkbenchItemEnvelope {
    return { itemId, kind, version: ITEM_STATES[kind].version, state };
}

/** The state a pane saved, when it was saved for this pane, at the version this build reads. */
export function decodeItemState<Kind extends SavedItemKind>(pane: { id: string; kind: Kind }, encoded: unknown): SavedItemStates[Kind] | null {
    const { version, decode } = ITEM_STATES[pane.kind];
    if (!isRecord(encoded) || encoded.itemId !== pane.id || encoded.kind !== pane.kind || encoded.version !== version) return null;
    return decode(encoded.state) as SavedItemStates[Kind] | null;
}
