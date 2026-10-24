import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import { DEFAULT_PREFERENCES, type CustomCommand, type Preferences, type ProjectRoot, type UpdateChannel } from "../api/generated/wire";
import { getState, setState, useStore, type StoreState } from "./store";
import { swallow } from "./toast";

/* Values set here that the core has not confirmed yet, laid over what it says until it does. */
const unconfirmed = new Map<keyof Preferences, unknown>();
const sending = new Set<keyof Preferences>();

/** Takes the preferences the core answered with. */
export function showPreferences(preferences: Preferences): void {
    setState({ preferences: { ...preferences, ...Object.fromEntries(unconfirmed) } });
}

/** Keeps the person's preferences as the core keeps them, for as long as the page runs. */
export function watchPreferences(): () => void {
    let live = true;
    const read = () =>
        coreCall<Preferences>("preferences")
            .then((preferences) => {
                if (live) showPreferences(preferences);
            })
            .catch(swallow("read your preferences"));
    const stop = onCoreChanged("preferences", () => void read());
    void read();
    return () => {
        live = false;
        stop();
    };
}

const NO_PROJECT_ROOTS: ProjectRoot[] = [];
export const projectRootsOf = (s: StoreState): ProjectRoot[] => s.preferences?.projectRoots ?? NO_PROJECT_ROOTS;
const NO_CUSTOM_COMMANDS: CustomCommand[] = [];
export const customCommandsOf = (s: StoreState): CustomCommand[] => s.preferences?.customCommands ?? NO_CUSTOM_COMMANDS;
const NO_PLUGIN_SETTINGS: Readonly<Record<string, unknown>> = {};
export const pluginSettingsOf = (s: StoreState): Readonly<Record<string, unknown>> => s.preferences?.pluginSettings ?? NO_PLUGIN_SETTINGS;

/** The person's preferences, or the core's defaults until it has said. */
export const preferencesOf = (s: StoreState): Preferences => s.preferences ?? DEFAULT_PREFERENCES;

/** The channel updates come from; stable until the core has said. */
export const updateChannelOf = (s: StoreState): UpdateChannel => preferencesOf(s).updateChannel;

/** A preference's value, or undefined until the core has said. */
export function usePreference<K extends keyof Preferences>(key: K): Preferences[K] | undefined {
    return useStore((s) => s.preferences?.[key]);
}

/** Shows `value` at once and sends the core only the latest of a burst, such as a pinch's. */
export function setPreference<K extends keyof Preferences>(key: K, value: Preferences[K]): void {
    unconfirmed.set(key, value);
    const shown = getState().preferences;
    if (shown) setState({ preferences: { ...shown, [key]: value } });
    if (!sending.has(key)) send(key);
}

function send(key: keyof Preferences): void {
    const value = unconfirmed.get(key);
    sending.add(key);
    const settled = () => {
        if (unconfirmed.get(key) === value) unconfirmed.delete(key);
    };
    coreCall<Preferences>("setPreference", { key, value })
        .then((preferences) => {
            settled();
            if (preferences) showPreferences(preferences);
        })
        .catch((error: unknown) => {
            settled();
            swallow("save your preference")(error);
            void coreCall<Preferences>("preferences").then(showPreferences, swallow("read your preferences"));
        })
        .finally(() => {
            sending.delete(key);
            if (unconfirmed.has(key)) send(key);
        });
}
