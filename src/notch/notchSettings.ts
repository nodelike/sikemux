import { invokeCommand as invoke } from "../api/invoke";
import { NOTCH_SETTING_CHOICES, NOTCH_SETTING_DEFAULTS } from "./generated/settings";

type Choices = typeof NOTCH_SETTING_CHOICES;

/** The Notch section of the settings, which the app hands to the `sikemux-notch` helper. */
export type NotchSettings = {
    readonly [Key in keyof typeof NOTCH_SETTING_DEFAULTS]: Key extends keyof Choices ? Choices[Key][number] : boolean;
};

export const DEFAULT_NOTCH_SETTINGS: NotchSettings = NOTCH_SETTING_DEFAULTS;

const isChoice = (key: keyof NotchSettings): key is keyof Choices => key in NOTCH_SETTING_CHOICES;

export function normaliseNotchSettings(value: unknown): NotchSettings {
    const saved = value !== null && typeof value === "object" ? (value as Record<string, unknown>) : {};
    const keys = Object.keys(DEFAULT_NOTCH_SETTINGS) as (keyof NotchSettings)[];
    return Object.fromEntries(
        keys.map((key) => {
            const allowed: readonly unknown[] = isChoice(key) ? NOTCH_SETTING_CHOICES[key] : [true, false];
            return [key, allowed.includes(saved[key]) ? saved[key] : DEFAULT_NOTCH_SETTINGS[key]];
        }),
    ) as NotchSettings;
}

export const notchApi = {
    configure: (settings: NotchSettings) => invoke<void>("notch_configure", { settings }),
};
