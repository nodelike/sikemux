import { setState } from "../state/store";
import { testPreferences } from "../test/preferences";

/** For tests: the preferences a core answers someone past the introduction with, so a plugin's settings read and save. */
export function answerPreferencesForTests(): void {
    setState({ preferences: testPreferences() });
}
