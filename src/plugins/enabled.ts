import { useMemo } from "react";
import type { PluginManifest } from "../api/plugins";
import { getState, useStore } from "../state/store";
import { frontendPlugin, frontendPlugins, type FrontendPlugin } from "./registry";

/** Built in and not switched off. A disabled plugin acts everywhere as if it were absent. */
export function isPluginEnabled(id: string, disabled: readonly string[] = getState().disabledPlugins): boolean {
    return !disabled.includes(id);
}

/** Every registered plugin the person has not switched off, for code outside React. */
export function enabledFrontendPlugins(): readonly FrontendPlugin[] {
    const { disabledPlugins } = getState();
    return frontendPlugins().filter((plugin) => isPluginEnabled(plugin.id, disabledPlugins));
}

export type BuiltPlugin = FrontendPlugin & { readonly manifest: PluginManifest };

/** Plugins compiled into this build on both sides, enabled or not, in the native side's order. */
export function useBuiltPlugins(): readonly BuiltPlugin[] {
    const manifests = useStore((s) => s.pluginManifests);
    return useMemo(
        () =>
            manifests.flatMap((manifest) => {
                const plugin = frontendPlugin(manifest.id);
                return plugin ? [{ ...plugin, manifest }] : [];
            }),
        [manifests],
    );
}
