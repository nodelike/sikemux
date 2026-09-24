import type { PluginManifest } from "../api/plugins";
import { isPluginKind, pluginIdOf } from "../plugins/kinds";
import type { SessionKind } from "./types";

export type RailGroup = "project" | "ssh" | "plugins" | "command";

export const RAIL_GROUP_ORDER: readonly RailGroup[] = ["project", "ssh", "plugins", "command"];

/** A session of a plugin this build lacks, or that is switched off, has no group, and the rail leaves it out. */
export function railGroupOf(kind: SessionKind, manifests: readonly PluginManifest[], disabled: readonly string[] = []): RailGroup | null {
    if (isPluginKind(kind)) {
        const id = pluginIdOf(kind);
        return manifests.some((manifest) => manifest.id === id) && !disabled.includes(id) ? "plugins" : null;
    }
    return kind;
}
