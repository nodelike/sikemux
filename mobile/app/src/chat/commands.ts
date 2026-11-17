import { commandMatches } from '@shared/chat/composerInput';
import type { ChatCommandInfo } from '@shared/wire.generated';

/** `/side`, which Sikemux answers itself by opening a side chat, ahead of the agent's own commands. */
export const SIDE_COMMAND: ChatCommandInfo = { name: 'side', description: "Ask aside, with this chat's context" };

/** The question after `/side`, empty when none, or undefined when the text is not `/side`. */
export function sideQuestion(text: string): string | undefined {
  return /^\/side(\s|$)/.test(text) ? text.slice(5).trim() : undefined;
}

/** The slash commands with what is typed anywhere in their name, while only a command is typed: Sikemux's own first. */
export function matchingCommands(commands: ChatCommandInfo[], typed: string): ChatCommandInfo[] {
  const needle = /^\/(\S*)$/.exec(typed)?.[1];
  const all = [SIDE_COMMAND, ...commands.filter((command) => command.name !== SIDE_COMMAND.name)];
  return needle === undefined ? [] : commandMatches(all, needle);
}
