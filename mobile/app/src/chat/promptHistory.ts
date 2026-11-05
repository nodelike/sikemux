import type { ChatMessage } from '@shared/chat/types';

/** What the person sent in this chat, oldest first, with a message repeated back to back kept once. */
export function sentPrompts(messages: readonly ChatMessage[], queued: readonly string[] = []): string[] {
  const prompts: string[] = [];
  const texts = messages
    .filter((message) => message.role === 'user' && !message.letters?.length)
    .map((message) =>
      message.parts
        .map((part) => (part.kind === 'text' ? part.text : ''))
        .join('')
        .trim(),
    );
  for (const text of [...texts, ...queued.map((text) => text.trim())]) {
    if (text && prompts.at(-1) !== text) prompts.push(text);
  }
  return prompts;
}
