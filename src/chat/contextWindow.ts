import { sessionConfigs } from "./ComposerPickers";

const DEFAULT_WINDOW = 200_000;
const LONG_WINDOW = 1_000_000;

/* Claude's transcript does not say how big its window is, and neither does the
   adapter until a turn reports it. This is the adapter's own guess: a model
   named for a 1M window has one, and every other has 200K. */
export function guessClaudeWindow(setup: Record<string, unknown>, fallbackModel?: string): number {
    const model = sessionConfigs(setup).find((config) => config.id === "model");
    const names = [model?.currentValue, model?.options.find((option) => option.value === model.currentValue)?.label, fallbackModel];
    return names.some((name) => name && /\b1m\b/i.test(name)) ? LONG_WINDOW : DEFAULT_WINDOW;
}
