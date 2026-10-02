import { useMemo } from "react";
import { useStore } from "../state/store";
import { isPluginEnabled, useBuiltPlugins, type BuiltPlugin } from "./enabled";

/** Plugins compiled into this build on both sides and not switched off. */
export function useInstalledPlugins(): readonly BuiltPlugin[] {
    const built = useBuiltPlugins();
    const disabled = useStore((s) => s.disabledPlugins);
    return useMemo(() => built.filter((plugin) => isPluginEnabled(plugin.id, disabled)), [built, disabled]);
}
