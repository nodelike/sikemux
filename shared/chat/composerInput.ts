import type { ChatCommandInfo } from "./types";

const MAX_COMMANDS = 8;

/** The agent's slash commands with `needle` anywhere in their name. */
export function commandMatches(commands: readonly ChatCommandInfo[], needle: string): ChatCommandInfo[] {
    const lower = needle.toLowerCase();
    return commands.filter((command) => command.name.toLowerCase().includes(lower)).slice(0, MAX_COMMANDS);
}

/** The command a message opens with that the session does not offer. A session that lists none is taken at its word. */
export function unknownCommand(text: string, commands: readonly ChatCommandInfo[]): string | null {
    const word = /^\/(\S+)/.exec(text)?.[1];
    /* Only a first word like `/name` counts, so a path such as `/usr/bin` goes to the agent as text. */
    const name = word && /^[\p{L}\p{N}_:.-]+$/u.test(word) ? word : null;
    return name && commands.length > 0 && !commands.some((command) => command.name === name) ? name : null;
}

/** How the draft names attachment `number`, counting from 1, at the place its picture belongs. */
export const imageTag = (number: number): string => `[Image #${number}]`;
