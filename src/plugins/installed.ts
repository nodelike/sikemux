import { useMemo } from "react";
import { useStore } from "../state/store";
import { isPluginEnabled, useBuiltPlugins } from "./enabled";
import type { FrontendPlugin } from "./registry";

/** Plugins compiled into this build on both sides and not switched off. */
export function useInstalledPlugins(): readonly FrontendPlugin[] {
    const built = useBuiltPlugins();
    const disabled = useStore((s) => s.disabledPlugins);
    return useMemo(() => built.filter((plugin) => isPluginEnabled(plugin.id, disabled)), [built, disabled]);
}
