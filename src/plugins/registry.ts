import type { ComponentType, ReactNode } from "react";
import type { CtxItem } from "../rail/FileTree";
import { isPluginKind, pluginIdOf, type PluginKind } from "./kinds";
import type { PluginSignIn } from "../plugin-api/signInState";

export interface PluginSurfaceProps {
    readonly paneId: string;
    readonly visible: boolean;
}

/** What a document tab in the workspace strip shows. */
export interface PluginDocumentTab {
    readonly label: string;
    readonly title?: string;
    readonly icon?: ReactNode;
    readonly dirty?: boolean;
}

/**
 * A surface that holds documents, each shown as a tab in the workspace strip
 * the way an editor's files are. Reads are plain functions of plugin state so
 * core can walk tabs outside React; `subscribe` says when to read again.
 */
export interface PluginDocuments {
    list(paneId: string): { readonly ids: readonly string[]; readonly activeId: string | null };
    describe(paneId: string, id: string): PluginDocumentTab;
    select(paneId: string, id: string): void;
    close(paneId: string, id: string): void;
    reorder?(paneId: string, sourceId: string, targetId: string, placement: "before" | "after"): void;
    menu?(paneId: string, id: string): readonly CtxItem[];
    subscribe(listener: () => void): () => void;
}

export interface PluginSurface {
    readonly kind: PluginKind;
    readonly title: string;
    readonly icon: (size: number) => ReactNode;
    readonly render: (props: PluginSurfaceProps) => ReactNode;
    /** What ⌘P does while this surface is in front, in place of the file finder. */
    readonly quickOpen?: () => void;
    readonly documents?: PluginDocuments;
}

export interface PluginTopBarProps {
    /** The folder of the project in front, or null when something else is. */
    readonly projectCwd: string | null;
    /** Whether the pointer is over the right of the top bar, where the item would appear. */
    readonly stripHovered: boolean;
}

/** A shortcut a plugin offers while it is in use; people can rebind it in Settings. */
export interface PluginShortcut {
    readonly name: string;
    readonly label: string;
    readonly detail: string;
    readonly defaultBinding: string;
    /** False when it does not apply right now, which leaves the key to whatever else wants it. */
    readonly run: () => boolean;
}

/** Something a plugin can open from the app's session switcher. */
export interface PluginPickerEntry {
    readonly id: string;
    readonly name: string;
    readonly sub: string;
    readonly icon: ReactNode;
    open(): void;
    /** Removes it from the list, where that means something. */
    forget?(): void;
}

export interface FrontendPlugin {
    readonly id: string;
    readonly surfaces: readonly PluginSurface[];
    readonly open: () => void;
    readonly openTitle: string;
    /** Its logo in its own colours: in Settings, and on each call an agent makes to its tools. */
    readonly mark?: (size: number) => ReactNode;
    /** Hosts whose links in the chat carry its mark. A leading dot takes every subdomain. */
    readonly linkHosts?: readonly string[];
    /** A default shortcut for `open`, like "Alt+KeyA"; people can change it in Settings. */
    readonly openShortcut?: string;
    readonly shortcuts?: readonly PluginShortcut[];
    /** Entries for the session switcher, under this heading. Read from plugin settings, so the switcher follows them. */
    readonly picker?: { readonly heading: string; entries(): readonly PluginPickerEntry[] };
    /** Always mounted; it decides for itself when to show. */
    readonly Overlay?: ComponentType;
    readonly TopBarItem?: ComponentType<PluginTopBarProps>;
    /** Whether the person is signed in to its service, for plugins that need a sign-in. */
    readonly useSignIn?: () => PluginSignIn;
}

const plugins = new Map<string, FrontendPlugin>();
const surfaces = new Map<PluginKind, PluginSurface>();

export function registerFrontendPlugin(plugin: FrontendPlugin): void {
    if (plugins.has(plugin.id)) throw new Error(`plugin ${plugin.id} is registered twice`);
    for (const surface of plugin.surfaces) {
        if (!isPluginKind(surface.kind) || pluginIdOf(surface.kind) !== plugin.id) {
            throw new Error(`plugin ${plugin.id} cannot own the surface kind ${surface.kind}`);
        }
    }
    plugins.set(plugin.id, plugin);
    for (const surface of plugin.surfaces) surfaces.set(surface.kind, surface);
}

export function frontendPlugin(id: string): FrontendPlugin | undefined {
    return plugins.get(id);
}

export function frontendPlugins(): readonly FrontendPlugin[] {
    return [...plugins.values()];
}

const HOST = /^https?:\/\/(?:[^@/?#]*@)?([^/?#:]+)/i;

/** The plugin whose service a link points at, so the link can wear its mark. */
export function linkPlugin(href: string): FrontendPlugin | undefined {
    const host = HOST.exec(href)?.[1]?.toLowerCase();
    if (!host) return undefined;
    for (const plugin of plugins.values()) {
        if (plugin.linkHosts?.some((owned) => (owned.startsWith(".") ? host.endsWith(owned) : host === owned))) return plugin;
    }
    return undefined;
}

export function pluginSurface(kind: string): PluginSurface | undefined {
    return isPluginKind(kind) ? surfaces.get(kind) : undefined;
}
