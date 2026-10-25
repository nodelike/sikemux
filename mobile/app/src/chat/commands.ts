import { commandMatches } from '@shared/chat/composerInput';
import type { ChatCommandInfo } from '@shared/chat/types';

/** The agent's slash commands with what is typed anywhere in their name, while only a command is typed. */
export function matchingCommands(commands: ChatCommandInfo[], typed: string): ChatCommandInfo[] {
  const needle = /^\/(\S*)$/.exec(typed)?.[1];
  return needle === undefined ? [] : commandMatches(commands, needle);
}
