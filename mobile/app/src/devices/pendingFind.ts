import type { FoundLine } from '@shared/wire.generated';

/** A chat that never showed lets its find go, so a later plain open does not find. */
const LAPSE_MS = 30_000;

let asked: { core: string; agent: string; words: string; at: number } | null = null;

/** Opens `agent`'s chat on the first words `line` found, once it shows. */
export function askFind(core: string, agent: string, line: FoundLine | undefined): void {
  const words = line?.runs.find((run) => run.found)?.text;
  asked = words ? { core, agent, words, at: Date.now() } : null;
}

/** The words to find in `agent`'s chat, once. */
export function takeFind(core: string, agent: string): string | undefined {
  if (!asked || asked.core !== core || asked.agent !== agent) return undefined;
  const { words, at } = asked;
  asked = null;
  return Date.now() - at < LAPSE_MS ? words : undefined;
}
