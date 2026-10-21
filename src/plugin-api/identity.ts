import type { PluginKind } from "../plugins/kinds";

/** Who a plugin is, as its manifest says: the id the core knows it by and the name people see. */
export interface PluginIdentity {
    readonly id: string;
    readonly name: string;
}

export const surfaceKind = (plugin: PluginIdentity, surface: string): PluginKind => `${plugin.id}:${surface}` as PluginKind;

/** A cached read that belongs to the plugin, kept under its id so the plugin can read all of its own again at once. */
export const resourceKind = (pluginId: string, name: string): string => `${pluginId}.${name}`;
