import { DEFAULT_PREFERENCES, type Preferences } from "@shared/wire.generated";

/** Preferences as a core answers them for someone past the introduction, with `changes` laid over. */
export function testPreferences(changes: Partial<Preferences> = {}): Preferences {
    return { ...DEFAULT_PREFERENCES, onboardingComplete: true, ...changes };
}
