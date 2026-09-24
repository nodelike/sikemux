import { pluginSurface } from "../plugins/registry";
import type { SessionKind } from "./types";

/** A plugin's session is one of a kind, so it goes by the plugin's own name. */
export function fixedSessionName(kind: SessionKind): string | undefined {
    return pluginSurface(kind)?.title;
}
