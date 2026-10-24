import { pluginSettingsOf, setPreference } from "../state/preferences";
import { getState, useStore } from "../state/store";

export interface PluginSettings<T> {
    useSelect<S>(select: (settings: T) => S): S;
    get(): T;
    update(recipe: (settings: T) => T): void;
}

const UNREAD = Symbol("unread");

/**
 * Settings saved with the workspace under the plugin's id. `decode` sees
 * whatever was saved, including nothing at all, and must return usable settings.
 */
export function definePluginSettings<T>(pluginId: string, decode: (saved: unknown) => T): PluginSettings<T> {
    let lastSaved: unknown = UNREAD;
    let lastDecoded: T;
    const read = (saved: unknown): T => {
        if (saved !== lastSaved) {
            lastSaved = saved;
            lastDecoded = decode(saved);
        }
        return lastDecoded;
    };
    return {
        useSelect: (select) => useStore((s) => select(read(pluginSettingsOf(s)[pluginId]))),
        get: () => read(pluginSettingsOf(getState())[pluginId]),
        update: (recipe) => {
            const all = pluginSettingsOf(getState());
            setPreference("pluginSettings", { ...all, [pluginId]: recipe(read(all[pluginId])) });
        },
    };
}
