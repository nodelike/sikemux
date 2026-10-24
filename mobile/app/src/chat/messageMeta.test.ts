import { describe, expect, it } from 'vitest';

import type { ChatMessage } from '@mac/chat/types';
import { stripOf, stripOwner } from './messageMeta';

const message = (id: string, role: ChatMessage['role'], text: string, fields: Partial<ChatMessage> = {}): ChatMessage => ({
  id,
  role,
  parts: [{ id: `${id}-text`, kind: 'text', text }],
  ...fields,
});

describe("a message's strip", () => {
  const messages = [
    message('u1', 'user', 'Fix it', { meta: { from: 'u1', at: 1_000 } }),
    message('a1', 'assistant', 'Looking.'),
    message('a2', 'assistant', 'Fixed.', { meta: { from: 'a1', at: 61_000, took: 60_000 } }),
  ];

  it('sits under a prompt, with its own text and time', () => {
    expect(stripOwner(messages, 'u1')).toBe('u1');
    expect(stripOf(messages, 'u1')).toMatchObject({ text: 'Fix it', at: 1_000 });
  });

  it('sits under the last of an answer, wherever in it the tap was, and covers all of it', () => {
    expect(stripOwner(messages, 'a1')).toBe('a2');
    expect(stripOf(messages, 'a2')).toMatchObject({ text: 'Looking.\n\nFixed.', at: 61_000, took: 60_000 });
  });

  it('belongs to no message that is gone', () => {
    expect(stripOwner(messages, 'gone')).toBeUndefined();
    expect(stripOf(messages, 'gone')).toBeNull();
  });
});
