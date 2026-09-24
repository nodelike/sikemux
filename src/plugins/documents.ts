import { useSyncExternalStore } from "react";
import { isPluginKind } from "./kinds";
import { frontendPlugins, pluginSurface, type PluginDocuments } from "./registry";

/** The documents a window of this role holds, when a plugin surface keeps them. */
export function pluginDocuments(role: string): PluginDocuments | undefined {
    return isPluginKind(role) ? pluginSurface(role)?.documents : undefined;
}

let version = 0;

function subscribeAll(listener: () => void): () => void {
    const stops = frontendPlugins()
        .flatMap((plugin) => plugin.surfaces)
        .flatMap((surface) =>
            surface.documents
                ? [
                      surface.documents.subscribe(() => {
                          version += 1;
                          listener();
                      }),
                  ]
                : [],
        );
    return () => stops.forEach((stop) => stop());
}

/** Changes whenever any plugin's documents do, so a component reading them renders again. */
export function usePluginDocumentsVersion(): number {
    return useSyncExternalStore(subscribeAll, () => version);
}
